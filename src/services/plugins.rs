//! Plugins: small widgets for the dashboard or the bar, each defined by a
//! folder in `<config>/plugins/` with a `plugin.json`, in the spirit of
//! waybar's custom modules:
//!
//! ```json
//! {
//!   "name": "Battery health",
//!   "icon": "battery_heart_variant",
//!   "exec": "health.ps1",
//!   "interval": 60000,
//!   "onClick": ["ms-settings:batterysaver"],
//!   "location": "dashboard"
//! }
//! ```
//!
//! `exec` / `onClick` are a script in the folder (.ps1 .py .js .bat .cmd .exe),
//! a command line run by `cmd /c`, or an argv array. The command's stdout is
//! the widget text; JSON output (`{"text", "tooltip", "icon", "class",
//! "percentage"}`) sets more fields. Each run gets its own thread, a timeout
//! and a capped output; runs of one plugin never overlap.
//!
//! Plugins run local code, so they only ever come from the user's own
//! plugins folder (imported dots never bring any).

use std::{
    collections::HashSet,
    fs,
    io::Read,
    os::windows::process::CommandExt,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{Arc, Mutex},
    thread,
    time::{Duration, Instant},
};

use crossbeam_channel::{Receiver, RecvTimeoutError, Sender};
use serde::Deserialize;
use serde_json::Value;

const CREATE_NO_WINDOW: u32 = 0x0800_0000;
const MAX_OUTPUT: usize = 16 * 1024;
const MIN_INTERVAL_MS: u64 = 1000;

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(untagged)]
pub enum CommandSpec {
    Line(String),
    Argv(Vec<String>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Location {
    #[default]
    Dashboard,
    Bar,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Manifest {
    pub name: String,
    #[serde(default = "default_icon")]
    pub icon: String,
    #[serde(default)]
    pub exec: Option<CommandSpec>,
    /// Fixed text for widgets without `exec` (launchers, shortcuts).
    #[serde(default)]
    pub text: String,
    #[serde(default)]
    pub on_click: Option<CommandSpec>,
    /// Milliseconds between runs; 0 runs once.
    #[serde(default = "default_interval")]
    pub interval: u64,
    #[serde(default = "default_timeout")]
    pub timeout: u64,
    #[serde(default)]
    pub location: Location,
}

fn default_icon() -> String {
    "extension".into()
}
fn default_interval() -> u64 {
    10_000
}
fn default_timeout() -> u64 {
    10_000
}

#[derive(Debug, Clone, PartialEq)]
pub struct Plugin {
    /// Folder name, used as the stable id (`plugins.disabled`).
    pub id: String,
    pub dir: PathBuf,
    pub manifest: Manifest,
}

impl Plugin {
    pub fn interval(&self) -> Option<Duration> {
        (self.manifest.interval > 0).then(|| Duration::from_millis(self.manifest.interval.max(MIN_INTERVAL_MS)))
    }
    pub fn timeout(&self) -> Duration {
        Duration::from_millis(self.manifest.timeout.clamp(500, 120_000))
    }
}

/// Reads every `<dir>/*/plugin.json`. Broken manifests are reported, not fatal.
pub fn discover(dir: &Path) -> (Vec<Plugin>, Vec<(String, String)>) {
    let mut plugins = Vec::new();
    let mut errors = Vec::new();
    let Ok(rd) = fs::read_dir(dir) else { return (plugins, errors) };
    let mut folders: Vec<PathBuf> = rd.flatten().map(|e| e.path()).filter(|p| p.join("plugin.json").is_file()).collect();
    folders.sort();
    for folder in folders {
        let id = folder.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let parsed = fs::read_to_string(folder.join("plugin.json"))
            .map_err(|e| e.to_string())
            .and_then(|t| serde_json::from_str::<Manifest>(&t).map_err(|e| e.to_string()));
        match parsed {
            Ok(m) if m.name.trim().is_empty() => errors.push((id, "\"name\" is empty".into())),
            Ok(manifest) => plugins.push(Plugin { id, dir: folder, manifest }),
            Err(e) => errors.push((id, e)),
        }
    }
    (plugins, errors)
}

/// Turns a command spec into an argv, resolving scripts inside `dir`.
pub fn resolve(spec: &CommandSpec, dir: &Path) -> Vec<String> {
    let local = |s: &str| {
        let p = dir.join(s.trim_start_matches("./").trim_start_matches(".\\"));
        if p.is_file() { p.display().to_string() } else { s.to_owned() }
    };
    match spec {
        CommandSpec::Argv(argv) => argv.iter().enumerate().map(|(i, a)| if i == 0 { local(a) } else { a.clone() }).collect(),
        CommandSpec::Line(line) => {
            let line = line.trim();
            let ext = Path::new(line).extension().and_then(|e| e.to_str()).map(str::to_ascii_lowercase);
            let is_single_path = !line.contains(' ') || dir.join(line).is_file();
            match ext.as_deref() {
                Some("ps1") if is_single_path => ["powershell", "-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-File"].iter().map(|s| s.to_string()).chain([local(line)]).collect(),
                Some("py") if is_single_path => vec!["python".into(), local(line)],
                Some("js") if is_single_path => vec!["node".into(), local(line)],
                Some("bat" | "cmd") if is_single_path => vec!["cmd".into(), "/c".into(), local(line)],
                Some("exe") if is_single_path => vec![local(line)],
                _ => vec!["cmd".into(), "/c".into(), line.to_owned()],
            }
        }
    }
}

/// How a widget should be tinted (`class` in JSON output).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Tone {
    #[default]
    Normal,
    Good,
    Warning,
    Critical,
}

impl Tone {
    pub fn id(self) -> &'static str {
        match self {
            Tone::Normal => "",
            Tone::Good => "good",
            Tone::Warning => "warning",
            Tone::Critical => "critical",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Output {
    pub text: String,
    pub tooltip: String,
    pub icon: Option<String>,
    pub tone: Tone,
    /// 0..=1 when the plugin reports a percentage (drawn as a bar).
    pub percentage: Option<f32>,
}

/// Parses a run's stdout: JSON object (waybar style) or plain text where
/// the first line is the text and the rest the tooltip.
pub fn parse_output(stdout: &str) -> Output {
    let trimmed = stdout.trim();
    if trimmed.starts_with('{')
        && let Ok(Value::Object(o)) = serde_json::from_str::<Value>(trimmed)
    {
        let s = |k: &str| o.get(k).and_then(Value::as_str).map(str::to_owned);
        let class = match o.get("class") {
            Some(Value::Array(a)) => a.iter().filter_map(Value::as_str).collect::<Vec<_>>().join(" "),
            Some(Value::String(c)) => c.clone(),
            _ => String::new(),
        };
        let tone = class.split_whitespace().fold(Tone::Normal, |t, c| match c {
            "critical" | "urgent" | "error" => Tone::Critical,
            "warning" if t != Tone::Critical => Tone::Warning,
            "good" | "ok" | "success" if t == Tone::Normal => Tone::Good,
            _ => t,
        });
        return Output {
            text: clip(&s("text").unwrap_or_default(), 200),
            tooltip: clip(&s("tooltip").unwrap_or_default(), 2000),
            icon: s("icon").filter(|i| !i.is_empty()),
            tone,
            percentage: o.get("percentage").and_then(Value::as_f64).map(|p| (p / 100.0).clamp(0.0, 1.0) as f32),
        };
    }
    let mut lines = trimmed.lines();
    let text = lines.next().unwrap_or_default().trim();
    let tooltip = lines.collect::<Vec<_>>().join("\n");
    Output { text: clip(text, 200), tooltip: clip(tooltip.trim(), 2000), ..Default::default() }
}

fn clip(s: &str, max: usize) -> String {
    if s.chars().count() <= max { s.to_owned() } else { s.chars().take(max - 1).collect::<String>() + "…" }
}

/// Runs `argv` in `dir` without a console window, killing it after `timeout`.
pub fn run(argv: &[String], dir: &Path, timeout: Duration) -> Result<String, String> {
    let (prog, args) = argv.split_first().ok_or("empty command")?;
    let mut child = Command::new(prog)
        .args(args)
        .current_dir(dir)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .creation_flags(CREATE_NO_WINDOW)
        .spawn()
        .map_err(|e| format!("{prog}: {e}"))?;
    // Drain the pipes on helper threads so a chatty child never blocks.
    let drain = |pipe: Option<Box<dyn Read + Send>>| {
        thread::spawn(move || {
            let mut buf = Vec::new();
            if let Some(p) = pipe {
                let _ = p.take(MAX_OUTPUT as u64).read_to_end(&mut buf);
            }
            String::from_utf8_lossy(&buf).into_owned()
        })
    };
    let out = drain(child.stdout.take().map(|p| Box::new(p) as Box<dyn Read + Send>));
    let err = drain(child.stderr.take().map(|p| Box::new(p) as Box<dyn Read + Send>));
    let started = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if started.elapsed() >= timeout => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!("timed out after {} s", timeout.as_secs_f32()));
            }
            Ok(None) => thread::sleep(Duration::from_millis(25)),
            Err(e) => return Err(e.to_string()),
        }
    };
    let (out, err) = (out.join().unwrap_or_default(), err.join().unwrap_or_default());
    if status.success() {
        Ok(out)
    } else {
        let detail = err.lines().chain(out.lines()).map(str::trim).find(|l| !l.is_empty()).unwrap_or_default().to_owned();
        Err(format!("exit code {}{}", status.code().unwrap_or(-1), if detail.is_empty() { String::new() } else { format!(": {detail}") }))
    }
}

/// One result for the UI; `generation` drops results from before a reload.
#[derive(Debug, Clone)]
pub struct Update {
    pub generation: u64,
    pub index: usize,
    pub result: Result<Output, String>,
}

enum Msg {
    Load(u64, Vec<Plugin>),
    RunNow(usize),
}

/// Runs the plugins on their intervals. Dropping it stops the scheduling.
pub struct Runner {
    tx: Sender<Msg>,
    generation: u64,
}

impl Runner {
    pub fn spawn(on_update: impl Fn(Update) + Send + Sync + 'static) -> Self {
        let (tx, rx) = crossbeam_channel::unbounded();
        let on_update = Arc::new(on_update);
        thread::Builder::new().name("plugins".into()).spawn(move || schedule(rx, on_update)).expect("spawn plugins");
        Self { tx, generation: 0 }
    }

    /// Replaces the running set; returns the generation results will carry.
    pub fn load(&mut self, plugins: Vec<Plugin>) -> u64 {
        self.generation += 1;
        let _ = self.tx.send(Msg::Load(self.generation, plugins));
        self.generation
    }

    pub fn run_now(&self, index: usize) {
        let _ = self.tx.send(Msg::RunNow(index));
    }
}

fn schedule(rx: Receiver<Msg>, on_update: Arc<dyn Fn(Update) + Send + Sync>) {
    let mut generation = 0;
    let mut plugins: Vec<Plugin> = Vec::new();
    let mut due: Vec<Option<Instant>> = Vec::new();
    let running: Arc<Mutex<HashSet<(u64, usize)>>> = Arc::default();
    loop {
        let now = Instant::now();
        // Start everything that is due and not already running.
        for (i, slot) in due.iter_mut().enumerate() {
            if slot.is_some_and(|t| t <= now) {
                let p = &plugins[i];
                *slot = p.interval().map(|d| now + d);
                if !running.lock().unwrap().insert((generation, i)) {
                    continue; // previous run still going: skip this tick
                }
                let (p, running, on_update) = (p.clone(), running.clone(), on_update.clone());
                thread::spawn(move || {
                    let result = match &p.manifest.exec {
                        Some(spec) => run(&resolve(spec, &p.dir), &p.dir, p.timeout()).map(|out| parse_output(&out)),
                        None => Ok(Output { text: p.manifest.text.clone(), ..Default::default() }),
                    };
                    running.lock().unwrap().remove(&(generation, i));
                    on_update(Update { generation, index: i, result });
                });
            }
        }
        let wait = due.iter().flatten().min().map(|t| t.saturating_duration_since(Instant::now())).unwrap_or(Duration::from_secs(3600));
        match rx.recv_timeout(wait) {
            Ok(Msg::Load(g, list)) => {
                generation = g;
                due = vec![Some(Instant::now()); list.len()];
                plugins = list;
            }
            Ok(Msg::RunNow(i)) => {
                if let Some(slot) = due.get_mut(i) {
                    *slot = Some(Instant::now());
                }
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => return,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("caelestia-plugins-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn parses_manifests_with_defaults() {
        let m: Manifest = serde_json::from_str(r#"{ "name": "Uptime", "exec": "uptime.ps1" }"#).unwrap();
        assert_eq!(m.icon, "extension");
        assert_eq!(m.interval, 10_000);
        assert_eq!(m.location, Location::Dashboard);
        assert_eq!(m.exec, Some(CommandSpec::Line("uptime.ps1".into())));
        let m: Manifest = serde_json::from_str(r#"{ "name": "X", "exec": ["git", "status"], "location": "bar", "onClick": "notepad", "interval": 0 }"#).unwrap();
        assert_eq!(m.exec, Some(CommandSpec::Argv(vec!["git".into(), "status".into()])));
        assert_eq!(m.location, Location::Bar);
        assert!(serde_json::from_str::<Manifest>(r#"{ "icon": "x" }"#).is_err(), "name is required");
    }

    #[test]
    fn bundled_examples_are_valid() {
        let (plugins, errors) = discover(&Path::new(env!("CARGO_MANIFEST_DIR")).join("examples").join("plugins"));
        assert!(errors.is_empty(), "{errors:?}");
        assert_eq!(plugins.iter().map(|p| p.id.as_str()).collect::<Vec<_>>(), ["disk", "uptime"]);
        assert_eq!(plugins[0].manifest.location, Location::Bar);
        assert_eq!(resolve(plugins[1].manifest.exec.as_ref().unwrap(), &plugins[1].dir)[0], "powershell");
    }

    #[test]
    fn discovers_plugins_and_reports_broken_ones() {
        let d = tmp("discover");
        for (dir, json) in [("b-good", r#"{ "name": "Good" }"#), ("a-broken", "{ nope"), ("c-noname", r#"{ "name": " " }"#)] {
            fs::create_dir_all(d.join(dir)).unwrap();
            fs::write(d.join(dir).join("plugin.json"), json).unwrap();
        }
        fs::create_dir_all(d.join("not-a-plugin")).unwrap();
        let (plugins, errors) = discover(&d);
        assert_eq!(plugins.iter().map(|p| p.id.as_str()).collect::<Vec<_>>(), ["b-good"]);
        assert_eq!(errors.iter().map(|e| e.0.as_str()).collect::<Vec<_>>(), ["a-broken", "c-noname"]);
        assert!(discover(&d.join("missing")).0.is_empty());
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn resolves_scripts_and_command_lines() {
        let d = tmp("resolve");
        fs::write(d.join("s.ps1"), "").unwrap();
        let s = |x: &str| CommandSpec::Line(x.into());
        let ps = resolve(&s("s.ps1"), &d);
        assert_eq!(ps[0], "powershell");
        assert_eq!(ps.last().unwrap(), &d.join("s.ps1").display().to_string());
        assert_eq!(resolve(&s("tool.py"), &d), ["python", "tool.py"]);
        assert_eq!(resolve(&s("echo hi there"), &d), ["cmd", "/c", "echo hi there"]);
        assert_eq!(resolve(&CommandSpec::Argv(vec!["./s.ps1".into(), "-x".into()]), &d), [d.join("s.ps1").display().to_string(), "-x".into()]);
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn parses_plain_and_json_output() {
        let o = parse_output("  42%\nCPU load\nmore\n");
        assert_eq!((o.text.as_str(), o.tooltip.as_str()), ("42%", "CPU load\nmore"));
        let o = parse_output(r#"{"text":"3 updates","tooltip":"git, rust","icon":"update","class":["warning"],"percentage":75}"#);
        assert_eq!(o.text, "3 updates");
        assert_eq!(o.icon.as_deref(), Some("update"));
        assert_eq!(o.tone, Tone::Warning);
        assert_eq!(o.percentage, Some(0.75));
        assert_eq!(parse_output(r#"{"text":"x","class":"good critical"}"#).tone, Tone::Critical);
        assert_eq!(parse_output(r#"{"text":"x","percentage":250}"#).percentage, Some(1.0));
        // Not JSON after all: plain text.
        assert_eq!(parse_output("{oops").text, "{oops");
        assert_eq!(parse_output(&"x".repeat(500)).text.chars().count(), 200);
    }

    #[test]
    fn runs_commands_with_output_errors_and_timeouts() {
        let d = std::env::temp_dir();
        let cmd = |s: &str| resolve(&CommandSpec::Line(s.into()), &d);
        assert_eq!(run(&cmd("echo hola"), &d, Duration::from_secs(10)).unwrap().trim(), "hola");
        let err = run(&cmd("exit 3"), &d, Duration::from_secs(10)).unwrap_err();
        assert!(err.starts_with("exit code 3"), "{err}");
        assert!(run(&["definitely-not-a-program-xyz".into()], &d, Duration::from_secs(5)).is_err());
        let started = Instant::now();
        let err = run(&cmd("ping -n 30 127.0.0.1"), &d, Duration::from_millis(600)).unwrap_err();
        assert!(err.contains("timed out"), "{err}");
        assert!(started.elapsed() < Duration::from_secs(10));
    }

    #[test]
    fn runner_reports_each_plugin_and_drops_old_generations() {
        let d = tmp("runner");
        let plugin = |id: &str, exec: Option<&str>, text: &str| Plugin {
            id: id.into(),
            dir: d.clone(),
            manifest: Manifest { name: id.into(), icon: default_icon(), exec: exec.map(|e| CommandSpec::Line(e.into())), text: text.into(), on_click: None, interval: 0, timeout: 10_000, location: Location::Dashboard },
        };
        let (tx, rx) = crossbeam_channel::unbounded();
        let mut runner = Runner::spawn(move |u| {
            let _ = tx.send(u);
        });
        let g = runner.load(vec![plugin("static", None, "fixed"), plugin("echo", Some("echo from-cmd"), "")]);
        let mut got: Vec<Update> = (0..2).map(|_| rx.recv_timeout(Duration::from_secs(20)).expect("update")).collect();
        got.sort_by_key(|u| u.index);
        assert!(got.iter().all(|u| u.generation == g));
        assert_eq!(got[0].result.as_ref().unwrap().text, "fixed");
        assert_eq!(got[1].result.as_ref().unwrap().text, "from-cmd");
        // interval 0 = run once; run_now triggers it again.
        runner.run_now(0);
        let again = rx.recv_timeout(Duration::from_secs(20)).expect("rerun");
        assert_eq!(again.index, 0);
        let _ = fs::remove_dir_all(&d);
    }
}
