fn main() {
    let args: Vec<std::ffi::OsString> = std::env::args_os().collect();
    let cwd = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
    let env = bistill_lib::Env::from_process();
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
