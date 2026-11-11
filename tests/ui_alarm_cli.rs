//! M448 (IDEAS §122, pepa #341) — `std/alarm`: el contrato en headless, idéntico en VM,
//! intérprete y nativo: permiso, programación con los tres disparadores, validación, pendientes,
//! la respuesta del usuario como evento `"alarm"` y las plataformas sin alarmas de sistema.
use std::path::PathBuf;
use std::process::Command;

fn tmp(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("raylang_test_alarm_{name}"));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn has_rustc() -> bool {
    Command::new("rustc").arg("--version").output().map(|o| o.status.success()).unwrap_or(false)
}

/// (nombre, stdout, stderr) del programa en cada motor disponible, con el entorno dado.
fn engines(dir: &PathBuf, env: &[(&str, &str)]) -> Vec<(String, String, String)> {
    let mut out = Vec::new();
    let run = |cmd: &mut Command| {
        cmd.current_dir(dir).env("RAY_UI_BACKEND", "headless").env("RAY_UI_TRACE", "1");
        for (k, v) in env {
            cmd.env(k, v);
        }
        let o = cmd.output().unwrap();
        assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
        (String::from_utf8_lossy(&o.stdout).into_owned(), String::from_utf8_lossy(&o.stderr).into_owned())
    };
    for (name, args) in [("vm", &["run", "prog.ray"][..]), ("interp", &["run", "--interp", "prog.ray"][..])] {
        let (o, e) = run(Command::new(env!("CARGO_BIN_EXE_ray")).args(args));
        out.push((name.to_string(), o, e));
    }
    if has_rustc() {
        let bin = dir.join("prog_bin");
        let b = Command::new(env!("CARGO_BIN_EXE_ray")).args(["build", "prog.ray", "--native", "-o", bin.to_str().unwrap()]).current_dir(dir).output().unwrap();
        assert!(b.status.success(), "build --native: {}", String::from_utf8_lossy(&b.stderr));
        assert!(!String::from_utf8_lossy(&b.stderr).contains("not supported in the native subset"));
        let (o, e) = run(&mut Command::new(&bin));
        out.push(("native".to_string(), o, e));
    }
    out
}

const PROG: &str = r#"import std/ui;
import std/alarm;

fn main() -> int {
    let w = ui.open("t", "about:blank", 300, 200).unwrap();
    print(alarm.status());
    print(alarm.request_permission());
    var a = alarm.message("Hora de tu medicamento");
    a.id = "med";
    a.sound = "melody";
    a.snooze_minutes = 10;
    a.stop_label = "Ya la tomé";
    a.data = "dose-1";
    print(alarm.schedule(a, alarm.Trigger.Daily(8, 0)));
    var s = alarm.message("Semanal");
    s.id = "week";
    print(alarm.schedule(s, alarm.Trigger.Weekly([5, 1, 3], 20, 30)));
    var once = alarm.message("Una vez");
    once.id = "once";
    print(alarm.schedule(once, alarm.Trigger.At(4102444800)));
    print(alarm.schedule(alarm.message(""), alarm.Trigger.Daily(8, 0)));
    print(alarm.schedule(alarm.message("x"), alarm.Trigger.At(5)));
    print(alarm.schedule(alarm.message("x"), alarm.Trigger.Weekly([9], 8, 0)));
    print(alarm.schedule(alarm.message("x"), alarm.Trigger.Daily(24, 0)));
    print(alarm.pending());
    var n = 0;
    while (n < 3) {
        match (ui.try_next_event()) {
            Option.Some(e) => match (alarm.event_response(e)) {
                Option.Some(r) => print("response " + r.id + " / " + r.action + " / " + r.data),
                Option.None => print("other " + e.kind),
            },
            Option.None => print("no event"),
        }
        n = n + 1;
    }
    print(alarm.cancel("med"));
    print(alarm.pending());
    print(alarm.cancel_all());
    print(alarm.pending());
    0
}
"#;

const WANT: &str = "Result.Ok(granted)
Result.Ok(granted)
Result.Ok(med)
Result.Ok(week)
Result.Ok(once)
Result.Err(alarm: an alarm needs a title)
Result.Err(alarm: At(…) is in the past)
Result.Err(alarm: weekday 9 is out of range (1 = Monday … 7 = Sunday))
Result.Err(alarm: Daily(24, 0) is not a valid time of day)
Result.Ok([med, week, once])
no event
no event
no event
Result.Ok(0)
Result.Ok([week, once])
Result.Ok(0)
Result.Ok([])
";

#[test]
fn alarms_schedule_validate_and_list_headless_on_all_engines() {
    let d = tmp("contract");
    std::fs::write(d.join("prog.ray"), PROG).unwrap();
    for (name, out, err) in engines(&d, &[]) {
        assert_eq!(out, WANT, "{name}: {err}");
        assert!(err.contains("[ui] alarm permission"), "{name}\n{err}");
        assert!(err.contains("[ui] alarm schedule med \"Hora de tu medicamento\" sound=\"melody\" snooze=10 stop=\"Ya la tomé\" max=0 trigger=daily:8:0"), "{name}\n{err}");
        assert!(err.contains("trigger=weekly:1,3,5:20:30") && err.contains("trigger=at:4102444800"), "{name}\n{err}");
        assert!(err.contains("[ui] alarm cancel med") && err.contains("[ui] alarm cancel all"), "{name}\n{err}");
    }
}

/// La respuesta inyectada llega como evento `"alarm"` con el id, la acción y los datos; una
/// alarma de una vez respondida con «stop» deja de estar pendiente, una pospuesta no.
#[test]
fn the_answer_arrives_as_an_alarm_event() {
    let d = tmp("response");
    std::fs::write(d.join("prog.ray"), PROG).unwrap();
    for (action, pending) in [("stop", "Result.Ok([med, week])"), ("snooze", "Result.Ok([med, week, once])")] {
        for (name, out, _err) in engines(&d, &[("RAY_UI_ALARM_RESPONSE", action)]) {
            let want = format!("response med / {action} / dose-1\nresponse week / {action} / \nresponse once / {action} / \n");
            assert!(out.contains(&want), "{name} {action}:\n{out}");
            assert!(out.contains(pending), "{name} {action}:\n{out}");
        }
    }
}

/// Sin alarmas de sistema (iOS por ahora, escritorio) y con el permiso denegado.
#[test]
fn unsupported_and_denied_are_reported() {
    let d = tmp("unsupported");
    std::fs::write(
        d.join("prog.ray"),
        "import std/ui;\nimport std/alarm;\nfn main() -> int {\n    let w = ui.open(\"t\", \"about:blank\", 300, 200).unwrap();\n    print(alarm.status());\n    print(alarm.request_permission().is_err());\n    print(alarm.schedule(alarm.message(\"x\"), alarm.Trigger.Daily(8, 0)).is_err());\n    print(alarm.pending());\n    0\n}\n",
    )
    .unwrap();
    for (name, out, _err) in engines(&d, &[("RAY_UI_ALARM_PERMISSION", "unsupported")]) {
        assert_eq!(out, "Result.Ok(unsupported)\ntrue\ntrue\nResult.Ok([])\n", "{name}");
    }
    for (name, out, _err) in engines(&d, &[("RAY_UI_ALARM_PERMISSION", "denied")]) {
        assert_eq!(out, "Result.Ok(denied)\nfalse\ntrue\nResult.Ok([])\n", "{name}");
    }
}

/// pepa #347: un id con `|` (y `#`, `:`) se programa, se lista y se cancela en los tres motores;
/// un tabulador sigue rechazado, con su motivo.
#[test]
fn an_id_with_a_pipe_is_accepted_on_all_engines() {
    let d = tmp("pipe_id");
    std::fs::write(
        d.join("prog.ray"),
        "import std/ui;\nimport std/alarm;\nfn main() -> int {\n    let w = ui.open(\"t\", \"about:blank\", 300, 200).unwrap();\n    var a = alarm.message(\"x\");\n    a.id = \"pepa:2026-11-11|d|m1|20:00#ab\";\n    print(alarm.schedule(a, alarm.Trigger.Daily(8, 0)));\n    print(alarm.pending());\n    print(alarm.cancel(a.id));\n    print(alarm.pending());\n    a.id = \"a\\tb\";\n    print(alarm.schedule(a, alarm.Trigger.Daily(8, 0)));\n    0\n}\n",
    )
    .unwrap();
    for (name, out, _err) in engines(&d, &[]) {
        assert_eq!(
            out,
            "Result.Ok(pepa:2026-11-11|d|m1|20:00#ab)\nResult.Ok([pepa:2026-11-11|d|m1|20:00#ab])\nResult.Ok(0)\nResult.Ok([])\nResult.Err(alarm: the id cannot contain tabs or newlines)\n",
            "{name}"
        );
    }
}
