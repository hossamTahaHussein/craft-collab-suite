//! `craft-suite-cli [--app docs|sheets|slides] COMMAND …`: the suite's command line. It runs the
//! editor's own (`wordcraft-cli`, `gridcraft-cli`, `deckcraft-cli`), so every command, `mcp` and
//! `collab-mcp --relay URL --token TOKEN` work for each app as they do there.

fn main() {
    let args: Vec<std::ffi::OsString> = std::env::args_os().skip(1).collect();
    if args.is_empty() || args.iter().any(|a| a == "--help" || a == "-h") && args.len() == 1 {
        println!(
            "craft-suite-cli [--app docs|sheets|slides] COMMAND …\n\n\
             Runs the command with Craft Docs' (the default), Craft Sheets' or Craft Slides' command line,\n\
             for example:\n\n  craft-suite-cli --app sheets collab-mcp --relay wss://relay.example --token T\n  \
             craft-suite-cli --app slides convert talk.pptx talk.pdf\n  craft-suite-cli open budget.xlsx\n\n\
             `open FILE` opens the file in the editor its extension names."
        );
        return;
    }
    if args.first().is_some_and(|a| a == "open") {
        let Some(file) = args.get(1).map(std::path::PathBuf::from) else {
            eprintln!("craft-suite-cli: open needs a file");
            std::process::exit(2);
        };
        let Some(editor) = craft_suite::editor_for(&file) else {
            eprintln!("craft-suite-cli: no Craft editor opens {}", file.display());
            std::process::exit(2);
        };
        match craft_suite::open_command(editor, Some(&file)).spawn() {
            Ok(_) => return,
            Err(e) => {
                eprintln!("craft-suite-cli: couldn't start {}: {e}", editor.name());
                std::process::exit(1);
            }
        }
    }
    let mut command = match craft_suite::cli_command(&args) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("craft-suite-cli: {e}");
            std::process::exit(2);
        }
    };
    match command.status() {
        Ok(status) => std::process::exit(status.code().unwrap_or(1)),
        Err(e) => {
            eprintln!("craft-suite-cli: couldn't run {}: {e}", command.get_program().to_string_lossy());
            std::process::exit(1);
        }
    }
}
