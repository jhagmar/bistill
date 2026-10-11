fn main() {
    let args: Vec<std::ffi::OsString> = std::env::args_os().collect();
    let cwd = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
    let env = bistill_lib::Env::from_process();
    if let Some(request) = bistill::tui_request(&args) {
        if std::io::IsTerminal::is_terminal(&std::io::stdout()) {
            let code = match request {
                Ok(tui) => attached(&cwd, &env, &tui),
                Err(text) => {
                    eprintln!("{text}");
                    1
                }
            };
            std::process::exit(code);
        }
        if args.len() > 1 {
            eprintln!("Open a terminal to start the inbox.");
            std::process::exit(1);
        }
    }
    let mut stdout = std::io::stdout();
    let mut stderr = std::io::stderr();
    let code = bistill::execute(
        &args,
        &cwd,
        &env,
        &mut stdout,
        &mut stderr,
        &mut bistill::Live,
    );
    std::process::exit(code);
}

fn attached(cwd: &std::path::Path, env: &bistill_lib::Env, tui: &bistill::Tui) -> i32 {
    let prepared = match bistill::prepare(cwd, &tui.flags, env, tui.verbose) {
        Ok(prepared) => prepared,
        Err(exit) => {
            eprint!("{}", exit.stderr);
            return exit.code;
        }
    };
    let mut terminal = match tui::Terminal::stdio() {
        Ok(terminal) => terminal,
        Err(err) => {
            eprintln!("{err}");
            return 1;
        }
    };
    let mut open = |url: &str| host::open_url(&bistill::browser(url));
    let exit = bistill::drive(&mut terminal, prepared, &mut bistill::Live, &mut open);
    drop(terminal);
    eprint!("{}", exit.stderr);
    exit.code
}
