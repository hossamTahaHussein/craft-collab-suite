//! The browser shell: web `Services`, drag-and-drop, and the eframe web runner.

use deckcraft_engine::Session;
use deckcraft_ui_egui::{Inbox, InboxEvent, InboxFile, Services, SlideApp};
use wasm_bindgen::JsCast as _;

const PRESENTATION_EXTS: &[&str] = &["deckcraft", "pptx", "potx", "ppsx", "txt", "md"];
const PICTURE_EXTS: &[&str] = &["png", "jpg", "jpeg", "gif", "webp", "bmp", "tif", "tiff"];
const AUDIO_EXTS: &[&str] = &["wav", "mp3", "m4a", "aac", "flac", "ogg", "opus", "aif", "aiff"];
const VIDEO_EXTS: &[&str] = &["mp4", "m4v", "mov", "webm"];
const CANVAS_ID: &str = "deckcraft_canvas";
const LOADING_ID: &str = "deckcraft_loading";

/// Start the browser shell: install logging, locate the canvas, launch the eframe runner,
/// build the sample or blank deck, and replace the loading element when launch finishes.
pub fn start() {
    eframe::WebLogger::init(log::LevelFilter::Info).ok();
    wasm_bindgen_futures::spawn_local(async {
        let Some(document) = web_sys::window().and_then(|w| w.document()) else {
            log::error!("no document");
            return;
        };
        let Some(canvas) = document.get_element_by_id(CANVAS_ID).and_then(|e| e.dyn_into::<web_sys::HtmlCanvasElement>().ok()) else {
            log::error!("missing <canvas id=\"{CANVAS_ID}\">");
            return;
        };
        let mut options = eframe::WebOptions::default();
        if query().contains("webgl")
            && let eframe::egui_wgpu::WgpuSetup::CreateNew(create) = &mut options.wgpu_options.wgpu_setup
        {
            create.instance_descriptor.backends = eframe::wgpu::Backends::GL;
        }
        // `?host=parent`: a same-origin page framing Craft Slides (the embed SDK) opens and saves
        // its files through `crate::host`.
        let page_bridge = if query_param("host").as_deref() == Some("parent") { crate::host::Bridge::new() } else { None };
        let app_bridge = page_bridge.clone();
        let result = eframe::WebRunner::new()
            .start(
                canvas,
                options,
                Box::new(move |cc| {
                    if let Some(rs) = &cc.wgpu_render_state {
                        log::info!("deckcraft-web {}: wgpu backend {:?}", env!("CARGO_PKG_VERSION"), rs.adapter.get_info().backend);
                    }
                    deckcraft_ui_egui::install_ime_pass_hooks(&cc.egui_ctx);
                    if let Some(token) = crate::presenter::token() {
                        let presenter = crate::presenter::Popup::new(token, cc.egui_ctx.clone()).map_err(std::io::Error::other)?;
                        return Ok(Box::new(presenter) as Box<dyn eframe::App>);
                    }
                    let inbox: Inbox = Inbox::default();
                    let bridge = app_bridge;
                    let mut services = services(inbox.clone(), cc.egui_ctx.clone());
                    if let Some(b) = &bridge {
                        b.connect(&mut services, cc.egui_ctx.clone());
                    }
                    let mut app = SlideApp::new(Session::new(), services);
                    let q = query();
                    if let Some(author) = query_param("author").filter(|_| bridge.is_some()) {
                        crate::host::Bridge::set_author(&mut app, &author);
                    }
                    if bridge.is_some() {
                        if let Err(e) = app.run("file.new", serde_json::json!({})) {
                            log::error!("new presentation: {e}");
                        }
                    } else if !q.contains("blank") {
                        if let Err(e) = deckcraft_engine::sample::open_sample(&mut app.session) {
                            log::error!("sample deck: {e}");
                        }
                    } else if let Err(e) = app.run("file.new", serde_json::json!({})) {
                        log::error!("new presentation: {e}");
                    }
                    if q.contains("show") {
                        app.start_show(0, false);
                    }
                    // `?collab=1` (with a host bridge): co-edit the presentation the page's relay shares.
                    let collab = (bridge.is_some() && query_param("collab").as_deref() == Some("1"))
                        .then(|| deckcraft_collab::link::CollabLink::new(query_param("docName")));
                    if let Some(b) = &bridge {
                        b.post_ready();
                    }
                    Ok(Box::new(WebShell {
                        app,
                        inbox,
                        bridge,
                        collab,
                        batch: Default::default(),
                        status: (0, None, 0),
                        watcher: Default::default(),
                    }))
                }),
            )
            .await;
        if let (Err(e), Some(bridge)) = (&result, &page_bridge) {
            bridge.post_failed(&format!("{e:?}"));
        }
        if let Some(el) = document.get_element_by_id(LOADING_ID) {
            match result {
                Ok(()) => el.remove(),
                Err(e) => el.set_inner_html(&format!("<p>DeckCraft failed to start: {e:?}</p><p>A browser with WebGPU or WebGL2 is required.</p>")),
            }
        }
    });
}

fn query() -> String {
    web_sys::window().and_then(|w| w.location().search().ok()).unwrap_or_default()
}

fn query_param(key: &str) -> Option<String> {
    web_sys::UrlSearchParams::new_with_str(&query()).ok()?.get(key)
}

/// Wraps the app to read dropped files asynchronously (browsers can't read them synchronously)
/// and feed them through the inbox.
struct WebShell {
    app: SlideApp,
    inbox: Inbox,
    bridge: Option<crate::host::Bridge>,
    /// Co-editing (`?collab=1`): the link between the replica and the page's relay.
    collab: Option<deckcraft_collab::link::CollabLink>,
    /// Other people's edits wait here to be applied together.
    batch: craft_collab_link::batch::RemoteBatch,
    /// The co-editing status last posted to the parent (pending edits, revision, people).
    status: (usize, Option<u64>, usize),
    /// What the page's JavaScript API has been told (`deckcraft:event`).
    watcher: crate::api::Watcher,
}

impl WebShell {
    /// The page's JavaScript API calls, and the relay's HTTP API requests (exports and commands)
    /// this editor answers. They run before the co-editing tick, so their edits go out this frame.
    fn page_api(&mut self) {
        let Some(bridge) = self.bridge.as_ref() else { return };
        for (id, method, params) in bridge.take_calls() {
            bridge.post_result(id, crate::api::call(&mut self.app, &method, &params));
        }
        let Some(link) = self.collab.as_mut() else { return };
        for request in link.take_host_requests() {
            use deckcraft_collab::link::{CollabLink, HostRequest};
            let answer = match request {
                HostRequest::Export { id, format } if format == "txt" => {
                    CollabLink::exported(id, crate::api::outline(&self.app).map(String::into_bytes))
                }
                HostRequest::Export { id, format } => CollabLink::exported(id, crate::api::document_file(&self.app, &format, None).map(|(_, b)| b)),
                HostRequest::Run { id, cmd, params, author } => CollabLink::ran(id, crate::api::run_as(&mut self.app, &cmd, params, &author)),
            };
            bridge.post_collab(&answer);
        }
    }

    /// Tell the page what changed this frame.
    fn page_events(&mut self) {
        let Some(bridge) = self.bridge.as_ref() else { return };
        let (rev, edited_by) = match self.collab.as_mut() {
            Some(link) => (link.rev().unwrap_or(0), link.take_edited_by()),
            None => (crate::api::local_rev(&self.app), Vec::new()),
        };
        let people: Vec<(String, u8)> = self.app.peers.iter().map(|p| (p.name.clone(), p.color)).collect();
        for (event, detail) in self.watcher.events(&self.app, rev, &edited_by, &people) {
            bridge.post_event(event, &detail);
        }
    }
}

impl eframe::App for WebShell {
    /// Delegate one eframe logic pass to the wrapped application.
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        let now = ctx.input(|i| i.time) * 1000.0;
        if let Some(bridge) = &self.bridge {
            for (name, bytes) in bridge.take_files() {
                let id = self.app.begin_external_file("open");
                push_inbox(&self.inbox, InboxEvent::File(InboxFile { id, name, bytes }));
            }
        }
        if let (Some(link), Some(bridge)) = (self.collab.as_mut(), self.bridge.as_ref()) {
            for msg in bridge.take_collab() {
                self.batch.push(msg);
            }
            for msg in self.batch.release(now) {
                for out in link.on_message(&msg, &mut self.app.session) {
                    bridge.post_collab(&out);
                }
            }
            if let Some(wait) = self.batch.wait(now) {
                ctx.request_repaint_after(std::time::Duration::from_secs_f64(wait / 1000.0));
            }
            link.take_document_ready();
        }
        self.app.logic(ctx);
        self.page_api();
        if let (Some(link), Some(bridge)) = (self.collab.as_mut(), self.bridge.as_ref()) {
            for out in link.tick(&mut self.app.session, now) {
                bridge.post_collab(&out);
            }
            if let Some(note) = link.take_note() {
                self.app.notify(note);
            }
            let peers = link.peers(&self.app.session);
            let status = (link.pending(), link.rev(), peers.len() + 1);
            self.app.peers = peers
                .into_iter()
                .map(|p| deckcraft_ui_egui::canvas::peers::RemotePeer { typing: p.typing(now), name: p.name, color: p.color, place: p.sel })
                .collect();
            if status != self.status {
                self.status = status;
                bridge.post_collab_status(status.0, status.1, status.2);
            }
            if link.presence_pending() {
                ctx.request_repaint_after(std::time::Duration::from_millis(100));
            }
            // Autosave: the relay chose this editor to save the shared presentation.
            if link.take_save_request() {
                match self.app.run("file.save", serde_json::json!({})) {
                    Ok(_) => bridge.post_collab(&link.saved()),
                    Err(e) => log::warn!("co-editing: autosave failed: {e}"),
                }
            }
            co_editing_ui(&mut self.app, link, bridge);
        }
        self.page_events();
        if let Some(bridge) = &self.bridge {
            bridge.sync(&self.app);
        }
        if self.app.show.is_none()
            && let Some(host) = self.app.services.presenter.as_mut()
        {
            host.close();
        }
    }

    /// Consume browser drops with fresh input, capture the identity before the asynchronous read,
    /// and deliver the id, name, and bytes through the inbox.
    fn raw_input_hook(&mut self, ctx: &egui::Context, raw: &mut egui::RawInput) {
        // Browsers cannot read dropped files synchronously. Consume this pass's drops here, with
        // fresh input, and capture the document/slide before the asynchronous read finishes.
        for f in std::mem::take(&mut raw.dropped_files) {
            let id = self.app.begin_external_file("drop");
            let inbox = self.inbox.clone();
            let ctx = ctx.clone();
            wasm_bindgen_futures::spawn_local(async move {
                let name = f.path().file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| "dropped".into());
                match f.bytes_async().await {
                    Ok(bytes) => {
                        push_inbox(&inbox, InboxEvent::File(InboxFile { id, name, bytes }));
                        ctx.request_repaint();
                    }
                    Err(e) => {
                        log::error!("couldn't read dropped file {name}: {e}");
                        push_inbox(&inbox, InboxEvent::Cancelled { id });
                        ctx.request_repaint();
                    }
                }
            });
        }
        self.app.raw_input_hook(ctx, raw);
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.app.ui(ui);
    }
}

/// Web services: no file paths, so `pick_open`/`pick_save`/`read` stay `None` and the UI falls
/// back to `open_async` (browser picker → inbox) and `download`.
fn services(inbox: Inbox, ctx: egui::Context) -> Services {
    let open_inbox = inbox.clone();
    let presenter = match crate::presenter::BrowserHost::new(ctx.clone()) {
        Ok(host) => Some(Box::new(host) as Box<dyn deckcraft_ui_egui::presenter::Host>),
        Err(error) => {
            log::error!("Presenter window: {error}");
            None
        }
    };
    Services {
        presenter,
        open_async: Some(Box::new(move |purpose: &str, request: u64| {
            let inbox = open_inbox.clone();
            let ctx = ctx.clone();
            let dialog = match purpose {
                "picture" => rfd::AsyncFileDialog::new().add_filter("Pictures", PICTURE_EXTS),
                "audio" => rfd::AsyncFileDialog::new().add_filter("Audio", AUDIO_EXTS),
                "video" => rfd::AsyncFileDialog::new().add_filter("Video", VIDEO_EXTS),
                _ => rfd::AsyncFileDialog::new().add_filter("Presentations", PRESENTATION_EXTS),
            };
            wasm_bindgen_futures::spawn_local(async move {
                let Some(file) = dialog.pick_file().await else {
                    push_inbox(&inbox, InboxEvent::Cancelled { id: request });
                    ctx.request_repaint();
                    return;
                };
                let bytes = file.read().await;
                push_inbox(&inbox, InboxEvent::File(InboxFile { id: request, name: file.file_name(), bytes }));
                ctx.request_repaint();
            });
        })),
        write: Some(Box::new(|name: &str, bytes: &[u8]| download(name, bytes))),
        download: Some(Box::new(|name: &str, bytes: &[u8]| {
            if let Err(e) = download(name, bytes) {
                log::error!("download of {name} failed: {e}");
            }
        })),
        inbox: Some(inbox),
        ..Default::default()
    }
}

fn push_inbox(inbox: &Inbox, event: InboxEvent) {
    inbox.lock().unwrap_or_else(|e| e.into_inner()).push(event);
}

/// Trigger a browser download of `bytes` named after the last component of `path`.
fn download(path: &str, bytes: &[u8]) -> Result<(), String> {
    let js = |e: wasm_bindgen::JsValue| format!("{e:?}");
    let name = std::path::Path::new(path).file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| "deckcraft".into());
    let window = web_sys::window().ok_or("no window")?;
    let document = window.document().ok_or("no document")?;
    let parts = js_sys::Array::of1(&js_sys::Uint8Array::from(bytes));
    let opts = web_sys::BlobPropertyBag::new();
    opts.set_type(mime_for(&name));
    let blob = web_sys::Blob::new_with_u8_array_sequence_and_options(&parts, &opts).map_err(js)?;
    let url = web_sys::Url::create_object_url_with_blob(&blob).map_err(js)?;
    let a: web_sys::HtmlAnchorElement = document.create_element("a").map_err(js)?.dyn_into().map_err(|_| "not an anchor")?;
    a.set_href(&url);
    a.set_download(&name);
    a.style().set_property("display", "none").map_err(js)?;
    let body = document.body().ok_or("no body")?;
    body.append_child(&a).map_err(js)?;
    a.click();
    a.remove();
    // Revoke after the click has been dispatched; the download keeps its own reference.
    let revoke = wasm_bindgen::closure::Closure::once_into_js(move || {
        web_sys::Url::revoke_object_url(&url).ok();
    });
    window.set_timeout_with_callback_and_timeout_and_arguments_0(revoke.unchecked_ref(), 10_000).map_err(js)?;
    Ok(())
}

fn mime_for(name: &str) -> &'static str {
    match name.rsplit('.').next().map(str::to_ascii_lowercase).as_deref() {
        Some("png") => "image/png",
        Some("jpg" | "jpeg") => "image/jpeg",
        Some("pdf") => "application/pdf",
        Some("gif") => "image/gif",
        Some("txt") => "text/plain",
        Some("pptx") => "application/vnd.openxmlformats-officedocument.presentationml.presentation",
        Some("deckcraft") => "application/zip",
        _ => "application/octet-stream",
    }
}

/// The co-editing UI: undo state, the save indicator, the read-only notice and Version history.
fn co_editing_ui(app: &mut SlideApp, link: &mut deckcraft_collab::link::CollabLink, bridge: &crate::host::Bridge) {
    use deckcraft_ui_egui::collab_ui::{CollabRequest, DeckPreview, SyncState, VersionRow};
    app.collab.active = link.rev().is_some();
    app.collab.can_undo = link.can_undo();
    app.collab.can_redo = link.can_redo();
    app.collab.offline = link.offline_locked();
    app.collab.connected = link.online();
    app.collab.sync = app.collab.active.then(|| SyncState::of(link.online(), link.pending()));
    if let Some(versions) = link.take_versions() {
        app.collab.set_versions(versions.into_iter().map(|v| VersionRow { rev: v.rev, at: v.at, users: v.users }).collect());
    }
    if let Some((rev, json)) = link.take_version_doc() {
        let deck = json.and_then(|json| <deckcraft_collab::SlidesDoc as craft_collab_link::LinkHost>::decode(&json));
        app.collab.set_preview(rev, DeckPreview::new(deck));
    }
    for request in app.take_collab_requests() {
        match request {
            CollabRequest::ListVersions => bridge.post_collab(&link.list_versions()),
            CollabRequest::Preview(rev) => bridge.post_collab(&link.get_version(rev)),
            CollabRequest::Restore(rev) => {
                link.restore(rev);
            }
            CollabRequest::SaveMyCopy => {
                let Some(d) = app.session.active() else { continue };
                let title = d.title();
                let stem = std::path::Path::new(&title).file_stem().map_or_else(|| title.clone(), |s| s.to_string_lossy().to_string());
                let name = format!("{stem} (my copy).pptx");
                let saved = deckcraft_pptx::export(&d.doc).map_err(|e| e.to_string()).and_then(|bytes| bridge.post_save(&name, &bytes));
                app.notify(saved.map_or_else(|e| e, |()| format!("Saved {name}")));
            }
        }
    }
}
