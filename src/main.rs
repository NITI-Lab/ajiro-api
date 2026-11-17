fn main() {
    quiet_broken_pipe();
    std::process::exit(ajiro_api::cli::main());
}

/// `ajiro-api list | head` closes stdout early. `println!` then panics with
/// "failed printing to stdout: Broken pipe"; exit quietly instead, like
/// other command-line tools do.
fn quiet_broken_pipe() {
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let msg = info
            .payload()
            .downcast_ref::<String>()
            .map(String::as_str)
            .or_else(|| info.payload().downcast_ref::<&str>().copied())
            .unwrap_or("");
        if msg.contains("Broken pipe") || msg.contains("os error 232") {
            // 232 = ERROR_NO_DATA ("The pipe is being closed") on Windows
            std::process::exit(0);
        }
        default_hook(info);
    }));
}
