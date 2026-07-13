use itertools::Itertools;
use kmiri_helper::*;
use std::{
    env, fs,
    io::{self, ErrorKind},
    path::PathBuf,
    process::Command,
};

const RUSTC_WRAPPER: &str = "kmiri-helper";
const CARGO_WRAPPER: &str = "cargo-kmiri-helper";

const ENV_RUSTC_WRAPPER: &str = "MY_WRAPPER";
const ENV_CARGO_WRAPPER: &str = "CARGO_MY_WRAPPER";

fn main() {
    let subprocess = is_subprocess();
    let state = if subprocess {
        ProcessState::subprocess()
    } else {
        ProcessState::main()
    };

    state.init_logger();

    // search CLI through environment variables, or just use the name if absent
    let rustc_wrapper = &*env::var(ENV_RUSTC_WRAPPER).unwrap_or_else(|_| RUSTC_WRAPPER.to_owned());
    let cargo_wrapper = &*env::var(ENV_CARGO_WRAPPER).unwrap_or_else(|_| CARGO_WRAPPER.to_owned());

    let args = std::env::args().collect::<Vec<_>>();

    if args
        .iter()
        .any(|arg| matches!(&**arg, "-vV" | "-Vv" | "-V" | "--version"))
    {
        // cargo invokes `rustc -vV` first
        state.spawn("rustc", &args[1..], &[]);
    } else if args
        .iter()
        .any(|arg| matches!(&**arg, "___" | "build_script_build" | "proc_macro"))
    {
        // cargo asks rustc via fake `--crate-name ___` to know compilation specifics;
        // don't analyze build.rs or proc_macro crates
        state.spawn("rustc", &args[1..], &[]);
    } else if subprocess {
        // cargo constructs full rustc arguments and forwards them to us,
        // now invoke our driver with these arguments
        state.spawn(rustc_wrapper, &args[1..], &[]);
    } else {
        // main process falls into this branch: basically call `cargo build` with hook
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
        state.recreate_dir_analysis();
        state.spawn("cargo", &args, &[("RUSTC", cargo_wrapper)]);
        state.merge_analysis();
    }
}

const ENV_INNER_SUBPROCESS: &str = "__KMIRI__SUBPROCESS";
fn is_subprocess() -> bool {
    std::env::var(ENV_INNER_SUBPROCESS).is_ok_and(|var| var == "1")
}

struct ProcessState {
    /// The cargo target directory.
    dir_target: PathBuf,
    /// The value is `${dir_target}/analysis.log`.
    log_file: PathBuf,
    /// The value is `${dir_target}/analysis`.
    dir_analysis: PathBuf,
}
impl ProcessState {
    /// Construct the state via cargo, and reset the analysis directory.
    fn main() -> Self {
        let cmd = "cargo metadata --format-version 1 | jq .target_directory -r";
        let output = Command::new("bash")
            .args(["-lc", cmd])
            .output()
            .unwrap_or_else(|err| panic!("Failed to run `{cmd}`: {err}"));
        assert!(output.status.success());

        let dir_target = PathBuf::from(std::str::from_utf8(&output.stdout).unwrap().trim());

        let dir_analysis = dir_target.join(ANALYSIS);
        rm(&dir_analysis);
        fs::create_dir_all(&dir_analysis)
            .unwrap_or_else(|err| panic!("Failed to create {dir_analysis:?}: {err}"));

        let log_file = dir_target.join(ANALYSIS_LOG);
        rm(&log_file);
        fs::File::create(&log_file)
            .unwrap_or_else(|err| panic!("Failed to create {log_file:?}: {err}"));

        ProcessState {
            dir_target,
            log_file,
            dir_analysis,
        }
    }

    /// Construct the state via the env var.
    fn subprocess() -> Self {
        let dir_target = PathBuf::from(env::var(ENV_INNER_DIR_TARGET).unwrap());
        ProcessState {
            log_file: dir_target.join(ANALYSIS_LOG),
            dir_analysis: dir_target.join(ANALYSIS),
            dir_target,
        }
    }

    /// The log file is appended.
    fn init_logger(&self) {
        let log_file = fs::OpenOptions::new()
            .append(true)
            .open(&self.log_file)
            .unwrap();
        env_logger::builder()
            .filter(None, log::LevelFilter::Debug)
            .write_style(env_logger::WriteStyle::Always)
            .target(env_logger::fmt::Target::Pipe(Box::new(log_file)))
            .init();
    }

    /// Spawn a subprocess.
    fn spawn(&self, cmd: &str, args: &[String], vars: &[(&str, &str)]) {
        let mut cmd = Command::new(cmd);

        // state related
        cmd.env(ENV_INNER_SUBPROCESS, "1")
            .env(ENV_INNER_DIR_TARGET, &self.dir_target);

        let status = cmd
            .args(args)
            .envs(vars.iter().copied())
            .stdout(io::stdout())
            .stderr(io::stderr())
            .spawn()
            .unwrap()
            .wait()
            .unwrap();
        if !status.success() {
            // panic!("[error] {cmd}: args={args:#?} vars={vars:?}");
            std::process::abort()
        }
    }

    /// Remove old json files.
    fn recreate_dir_analysis(&self) {
        let dir_analysis = &self.dir_analysis;
        if let Err(err) = std::fs::remove_dir_all(dir_analysis)
            && err.kind() != ErrorKind::NotFound
        {
            panic!("Directory {dir_analysis:?} can't be removed.")
        }
        std::fs::create_dir(dir_analysis).unwrap();
    }

    /// Merge all analysis json files.
    fn merge_analysis(&self) {
        let v_fn: Vec<FunctionInstanceInfo> = fs::read_dir(&self.dir_analysis)
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .filter(|path| path.extension() == Some("json".as_ref()))
            .sorted()
            .dedup()
            .flat_map(|path| {
                serde_json::from_reader::<_, Vec<FunctionInstanceInfo>>(
                    &fs::File::open(dbg!(path)).unwrap(),
                )
                .unwrap()
            })
            .collect();

        let out = fs::File::create(self.dir_target.join(ANALYSIS_JSON)).unwrap();
        serde_json::to_writer_pretty(out, &v_fn).unwrap();
    }
}

/// Remove a file or directory silently.
fn rm(path: &PathBuf) {
    let result = if path.is_file() {
        fs::remove_file(path)
    } else if path.is_dir() {
        fs::remove_dir_all(path)
    } else if !path.exists() {
        // Do nothing.
        return;
    } else {
        unimplemented!()
    };
    if let Err(err) = result {
        panic!("Failed to remove {path:?}: {err}")
    }
}
