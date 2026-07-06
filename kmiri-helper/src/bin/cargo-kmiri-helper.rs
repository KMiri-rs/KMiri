use kmiri_helper::FunctionInstanceInfo;
use std::{env::var, process::Command};

const RUSTC_WRAPPER: &str = "kmiri-helper";
const CARGO_WRAPPER: &str = "cargo-kmiri-helper";

const ENV_RUSTC_WRAPPER: &str = "MY_WRAPPER";
const ENV_CARGO_WRAPPER: &str = "CARGO_MY_WRAPPER";

fn main() {
    // let log_file_name = var("LOG_FILE").unwrap();
    // let log_file = std::fs::OpenOptions::new()
    //     .append(true)
    //     .open(log_file_name)
    //     .unwrap();
    // env_logger::builder()
    //     .filter(None, log::LevelFilter::Debug)
    //     .write_style(env_logger::WriteStyle::Always)
    //     .target(env_logger::fmt::Target::Pipe(Box::new(log_file)))
    //     .init();

    // Search CLI through environment variables, or just use the name if absent.
    let rustc_wrapper = &*var(ENV_RUSTC_WRAPPER).unwrap_or_else(|_| RUSTC_WRAPPER.to_owned());
    let cargo_wrapper = &*var(ENV_CARGO_WRAPPER).unwrap_or_else(|_| CARGO_WRAPPER.to_owned());

    let args = std::env::args().collect::<Vec<_>>();

    if args.len() == 2 && args[1].as_str() == "-vV" {
        // cargo invokes `rustc -vV` first
        run("rustc", &["-vV".to_owned()], &[]);
    } else if args.iter().any(|arg| arg == "___") {
        run("rustc", &args[1..], &[]);
    } else if std::env::var("WRAPPER").is_ok_and(|wrapper| wrapper == "1") {
        run(rustc_wrapper, &args[1..], &[]);
    } else {
        let mut args = args;
        if args[0].ends_with(cargo_wrapper) {
            if args.get(1).map(|arg| arg == rustc_wrapper).unwrap_or(false) {
                // [cargo, safety-tool, args...]
                args.remove(0);
            }
            args[0] = "build".to_owned();
        } else {
            unimplemented!("Need to support this case: {args:#?}")
        }
        // cargo build args...
        run(
            "cargo",
            &args,
            &[("RUSTC", cargo_wrapper), ("WRAPPER", "1")],
        );
        merge_analysis();
    }
}

fn run(cmd: &str, args: &[String], vars: &[(&str, &str)]) {
    let status = Command::new(cmd)
        .args(args)
        .envs(vars.iter().copied())
        .stdout(std::io::stdout())
        .stderr(std::io::stderr())
        .spawn()
        .unwrap()
        .wait()
        .unwrap();
    if !status.success() {
        // panic!("[error] {cmd}: args={args:#?} vars={vars:?}");
        std::process::abort()
    }
}

fn merge_analysis() {
    let dir_analysis = std::env::var("DIR_ANALYSIS").unwrap();
    let mut v_fn: Vec<FunctionInstanceInfo> = std::fs::read_dir(dir_analysis)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| path.extension() == Some("json".as_ref()))
        .flat_map(|path| {
            serde_json::from_reader::<_, Vec<FunctionInstanceInfo>>(
                &std::fs::File::open(dbg!(path)).unwrap(),
            )
            .unwrap()
        })
        .collect();
    kmiri_helper::dedup(&mut v_fn);
    let out = std::fs::File::create("analysis.json").unwrap();
    serde_json::to_writer_pretty(out, &v_fn).unwrap();
}
