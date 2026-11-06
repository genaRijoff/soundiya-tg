//! soundiya-tg: a Telegram remote for soundiya, running on the speaker next to it.
//! It only connects out (long polling), so nothing listens on the network.
mod access;
mod bot;
mod config;
mod soundiya;
mod telegram;
mod updates;
mod view;
mod voice;

use std::{path::PathBuf, process::ExitCode, sync::Arc};

const USAGE: &str = "\
soundiya-tg — Telegram remote for soundiya

  soundiya-tg run      start the bot (the init script does this)
  soundiya-tg check    test the token and the connection to soundiya
  soundiya-tg version

Options: --config PATH (default /data/opt/soundiya-tg/config.json)
         --socket PATH (default /run/soundiya.control)";

/// Exit code for a setup error (config, token): run.sh waits longer before the next try,
/// so a wrong token does not hammer Telegram every few seconds.
const EXIT_SETUP: u8 = 3;

fn main() -> ExitCode {
    let mut config_path = PathBuf::from(config::DEFAULT_PATH);
    let mut socket = PathBuf::from(soundiya::DEFAULT_SOCKET);
    let mut command = None;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--config" => match args.next() {
                Some(path) => config_path = path.into(),
                None => return fail("missing --config path"),
            },
            "--socket" => match args.next() {
                Some(path) => socket = path.into(),
                None => return fail("missing --socket path"),
            },
            other if command.is_none() => command = Some(other.to_owned()),
            other => return fail(&format!("unexpected argument '{other}'\n\n{USAGE}")),
        }
    }
    match command.as_deref() {
        Some("run") => run(config_path, socket),
        Some("check") => check(config_path, socket),
        Some("version" | "--version") => {
            println!("soundiya-tg {}", env!("CARGO_PKG_VERSION"));
            ExitCode::SUCCESS
        }
        None | Some("help" | "--help" | "-h") => {
            println!("{USAGE}");
            ExitCode::SUCCESS
        }
        Some(other) => fail(&format!("unknown command '{other}'\n\n{USAGE}")),
    }
}

fn fail(message: &str) -> ExitCode {
    eprintln!("soundiya-tg: {message}");
    ExitCode::FAILURE
}

fn runtime() -> Result<tokio::runtime::Runtime, String> {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| e.to_string())
}

fn check(config_path: PathBuf, socket: PathBuf) -> ExitCode {
    let result = config::Config::load(&config_path).and_then(|config| {
        println!("config: ok, {} allowed user(s)", config.users.len());
        let api = telegram::Api::new(&config.api, &config.token)?;
        runtime()?.block_on(bot::check(&api, &socket))
    });
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => fail(&e),
    }
}

fn run(config_path: PathBuf, socket: PathBuf) -> ExitCode {
    let runtime = match runtime() {
        Ok(runtime) => runtime,
        Err(e) => return fail(&e),
    };
    let result = runtime.block_on(async {
        let config = config::Config::load(&config_path)?;
        let api = telegram::Api::new(&config.api, &config.token)?;
        let me = bot::wait_for_telegram(&api).await?;
        eprintln!("bot: running as @{}", me.username.unwrap_or_default());
        if let Err(e) = api.set_commands(view::COMMANDS).await {
            eprintln!("telegram: command menu: {e}");
        }
        let seen = config_path.with_file_name("updates.json");
        let access = config_path.with_file_name("access.json");
        voice::reset_dir();
        let bot = Arc::new(bot::Bot::new(api, config, socket, seen, access));
        tokio::spawn(bot.clone().watch());
        tokio::spawn(bot.clone().watch_updates());
        let mut terminate =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
                .map_err(|e| e.to_string())?;
        tokio::select! {
            result = bot.poll() => result,
            _ = terminate.recv() => Ok(()),
            _ = tokio::signal::ctrl_c() => Ok(()),
        }
    });
    match result {
        Ok(()) => {
            eprintln!("bot: stopped");
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("soundiya-tg: {e}");
            ExitCode::from(EXIT_SETUP)
        }
    }
}
