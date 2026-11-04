//! The `postMessage` bridge to a same-origin parent page (`?host=parent`), for pages that embed
//! Craft Slides in a frame (the embed SDK):
//! - `deckcraft:ready` (to the parent): `{ version }`, the app is listening; send a file now.
//! - `deckcraft:open` (from the parent): `{ name, bytes }`, bytes an `ArrayBuffer` or
//!   `Uint8Array`, opened like a dropped file.
//! - `deckcraft:save` (to the parent): `{ name, bytes }` (a transferred `ArrayBuffer`) for every
//!   save and export, instead of a download.
//! - `deckcraft:dirty` (to the parent): `{ dirty }` whenever unsaved changes appear or go away.
//! - `deckcraft:failed` (to the parent): `{ error }` when the app couldn't start.
//! - `deckcraft:collab` (both ways, with `?collab=1`): `{ msg }`, one co-editing relay message
//!   as a JSON string (`craft_collab_core::proto`); the parent carries them to and from the relay.
//! - `deckcraft:collab.status` (to the parent): `{ pending, rev, people }` when any changes.
//! - `deckcraft:call` (from the parent): `{ id, method, params }`, a JavaScript API call
//!   (`crate::api::call`); answered with `deckcraft:result` `{ id, ok: true, result }` (and
//!   `name`, `bytes` for a file) or `{ id, ok: false, error }`.
//! - `deckcraft:event` (to the parent): `{ event, detail }`, what changed this frame
//!   (`crate::api::Watcher`).

use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::rc::Rc;

use deckcraft_ui_egui::{Services, SlideApp};
use wasm_bindgen::JsCast as _;
use wasm_bindgen::JsValue;
use wasm_bindgen::closure::Closure;

/// The longest user name taken from `?author=`.
const MAX_AUTHOR_CHARS: usize = 200;

#[derive(Clone)]
pub struct Bridge {
    window: web_sys::Window,
    parent: web_sys::Window,
    origin: String,
    /// Files from the parent, for the next frame (the app starts each one).
    files: Rc<RefCell<VecDeque<(String, Vec<u8>)>>>,
    /// Co-editing relay messages from the parent, for the next frame.
    /// API calls from the parent (id, method, params), waiting for the next frame.
    calls: Rc<RefCell<std::collections::VecDeque<(f64, String, serde_json::Value)>>>,
    collab: Rc<RefCell<VecDeque<String>>>,
    /// The unsaved-changes state last reported to the parent.
    reported: Rc<Cell<Option<bool>>>,
}

impl Bridge {
    /// `None` when the page isn't framed (there is no parent to talk to).
    pub fn new() -> Option<Self> {
        let window = web_sys::window()?;
        let parent = window.parent().ok().flatten()?;
        if js_sys::Object::is(&parent, &window) {
            return None;
        }
        let origin = window.location().origin().ok()?;
        Some(Self { window, parent, origin, files: Rc::default(), collab: Rc::default(), reported: Rc::default(), calls: Rc::default() })
    }

    /// Send saved and exported files to the parent instead of downloading them, and hear files
    /// and co-editing messages from it.
    pub fn connect(&self, services: &mut Services, ctx: egui::Context) {
        self.listen(ctx);
        let bridge = self.clone();
        services.download = Some(Box::new(move |name: &str, bytes: &[u8]| {
            if let Err(e) = bridge.post_save(name, bytes) {
                log::error!("couldn't hand {name} to the page: {e}");
            }
        }));
        let bridge = self.clone();
        services.write = Some(Box::new(move |name: &str, bytes: &[u8]| bridge.post_save(name, bytes)));
    }

    /// `?author=` names the person (comments, co-editing).
    pub fn set_author(app: &mut SlideApp, author: &str) {
        let author: String = author.trim().chars().take(MAX_AUTHOR_CHARS).collect();
        if !author.is_empty() {
            app.session.prefs.author = author;
        }
    }

    pub fn post_ready(&self) {
        self.send(&[("type", "deckcraft:ready".into()), ("version", env!("CARGO_PKG_VERSION").into())]);
    }

    pub fn post_failed(&self, error: &str) {
        self.send(&[("type", "deckcraft:failed".into()), ("error", error.into())]);
    }

    /// Files the parent sent since the last call.
    pub fn take_files(&self) -> Vec<(String, Vec<u8>)> {
        self.files.borrow_mut().drain(..).collect()
    }

    /// The co-editing messages the parent sent since the last call.
    pub fn take_collab(&self) -> Vec<String> {
        self.collab.borrow_mut().drain(..).collect()
    }

    /// Send a co-editing message to the relay, through the parent.
    pub fn post_collab(&self, msg: &str) {
        self.send(&[("type", "deckcraft:collab".into()), ("msg", msg.into())]);
    }

    /// The API calls the parent sent since the last frame.
    pub fn take_calls(&self) -> Vec<(f64, String, serde_json::Value)> {
        self.calls.borrow_mut().drain(..).collect()
    }

    /// Answer API call `id`; a file goes as a transferred `ArrayBuffer`.
    pub fn post_result(&self, id: f64, reply: Result<crate::api::Reply, String>) {
        let id = JsValue::from_f64(id);
        let sent = match reply {
            Ok(crate::api::Reply { result, file }) => {
                let result = js_sys::JSON::parse(&result.to_string()).unwrap_or(JsValue::NULL);
                let mut fields = vec![("type", "deckcraft:result".into()), ("id", id.clone()), ("ok", true.into()), ("result", result)];
                match file {
                    Some((name, bytes)) => {
                        let buffer = js_sys::Uint8Array::from(bytes.as_slice()).buffer();
                        fields.push(("name", name.as_str().into()));
                        fields.push(("bytes", buffer.clone().into()));
                        message(&fields)
                            .is_some_and(|m| self.parent.post_message_with_transfer(&m, &self.origin, &js_sys::Array::of1(&buffer)).is_ok())
                    }
                    None => self.send(&fields),
                }
            }
            Err(error) => {
                self.send(&[("type", "deckcraft:result".into()), ("id", id.clone()), ("ok", false.into()), ("error", error.as_str().into())])
            }
        };
        if !sent {
            // The page still hears that the call failed, instead of waiting for its timeout.
            log::warn!("api: couldn't answer a call");
            let _ = self.send(&[("type", "deckcraft:result".into()), ("id", id), ("ok", false.into()), ("error", "couldn't send the result".into())]);
        }
    }

    /// Tell the parent what changed (`deckcraft:event`).
    pub fn post_event(&self, event: &str, detail: &serde_json::Value) {
        let detail = js_sys::JSON::parse(&detail.to_string()).unwrap_or(JsValue::NULL);
        self.send(&[("type", "deckcraft:event".into()), ("event", event.into()), ("detail", detail)]);
    }

    /// Tell the parent how co-editing stands.
    pub fn post_collab_status(&self, pending: usize, rev: Option<u64>, people: usize) {
        self.send(&[
            ("type", "deckcraft:collab.status".into()),
            ("pending", (pending as f64).into()),
            ("rev", rev.map_or(JsValue::NULL, |r| (r as f64).into())),
            ("people", (people as f64).into()),
        ]);
    }

    /// Report unsaved changes when they appear or go away.
    pub fn sync(&self, app: &SlideApp) {
        let dirty = app.session.active().is_some_and(|d| !std::sync::Arc::ptr_eq(&d.doc, &d.saved_doc));
        if self.reported.get() != Some(dirty) && self.send(&[("type", "deckcraft:dirty".into()), ("dirty", dirty.into())]) {
            self.reported.set(Some(dirty));
        }
    }

    pub fn post_save(&self, path: &str, bytes: &[u8]) -> Result<(), String> {
        let name = std::path::Path::new(path).file_name().map_or_else(|| "Presentation.pptx".to_string(), |n| n.to_string_lossy().to_string());
        let buffer = js_sys::Uint8Array::from(bytes).buffer();
        let message = message(&[("type", "deckcraft:save".into()), ("name", name.as_str().into()), ("bytes", buffer.clone().into())])
            .ok_or_else(|| "couldn't build the message".to_string())?;
        self.parent.post_message_with_transfer(&message, &self.origin, &js_sys::Array::of1(&buffer)).map_err(|e| format!("{e:?}"))
    }

    fn listen(&self, ctx: egui::Context) {
        let parent = self.parent.clone();
        let origin = self.origin.clone();
        let files = self.files.clone();
        let collab = self.collab.clone();
        let calls = self.calls.clone();
        let on_message = Closure::<dyn FnMut(web_sys::MessageEvent)>::new(move |event: web_sys::MessageEvent| {
            if event.origin() != origin || !event.source().is_some_and(|s| js_sys::Object::is(&s, &parent)) {
                return;
            }
            let data = event.data();
            match string(&data, "type").as_deref() {
                Some("deckcraft:open") => {
                    let name = string(&data, "name").filter(|n| !n.trim().is_empty()).unwrap_or_else(|| "Presentation.pptx".to_string());
                    let Some(bytes) = bytes(&data) else { return };
                    files.borrow_mut().push_back((name, bytes));
                }
                Some("deckcraft:call") => {
                    let id = js_sys::Reflect::get(&data, &JsValue::from_str("id")).ok().and_then(|v| v.as_f64());
                    let method = string(&data, "method");
                    // No params is `{}`; anything but an object (or something that can't be read as
                    // JSON) is passed on as null, which the call refuses.
                    let params = match js_sys::Reflect::get(&data, &JsValue::from_str("params")) {
                        Ok(p) if p.is_undefined() || p.is_null() => serde_json::json!({}),
                        Ok(p) if p.is_object() && !js_sys::Array::is_array(&p) => js_sys::JSON::stringify(&p)
                            .ok()
                            .and_then(|p| p.as_string())
                            .and_then(|p| serde_json::from_str(&p).ok())
                            .unwrap_or(serde_json::Value::Null),
                        _ => serde_json::Value::Null,
                    };
                    if let (Some(id), Some(method)) = (id, method) {
                        calls.borrow_mut().push_back((id, method, params));
                        ctx.request_repaint();
                    }
                    return;
                }
                Some("deckcraft:collab") => {
                    let Some(msg) = string(&data, "msg") else { return };
                    collab.borrow_mut().push_back(msg);
                }
                _ => return,
            }
            ctx.request_repaint();
        });
        if self.window.add_event_listener_with_callback("message", on_message.as_ref().unchecked_ref()).is_ok() {
            // The listener lives as long as the page.
            on_message.forget();
        }
    }

    /// Post a message to the parent; false if it couldn't be sent.
    fn send(&self, fields: &[(&str, JsValue)]) -> bool {
        message(fields).is_some_and(|m| self.parent.post_message(&m, &self.origin).is_ok())
    }
}

fn message(fields: &[(&str, JsValue)]) -> Option<js_sys::Object> {
    let o = js_sys::Object::new();
    for (k, v) in fields {
        js_sys::Reflect::set(&o, &(*k).into(), v).ok()?;
    }
    Some(o)
}

fn string(data: &JsValue, key: &str) -> Option<String> {
    js_sys::Reflect::get(data, &key.into()).ok()?.as_string()
}

fn bytes(data: &JsValue) -> Option<Vec<u8>> {
    let v = js_sys::Reflect::get(data, &"bytes".into()).ok()?;
    if let Some(buf) = v.dyn_ref::<js_sys::ArrayBuffer>() {
        return Some(js_sys::Uint8Array::new(buf).to_vec());
    }
    v.dyn_ref::<js_sys::Uint8Array>().map(js_sys::Uint8Array::to_vec)
}
