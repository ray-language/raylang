//! M442 (IDEAS §121): `std/notify` en headless, en los tres motores — permiso, canales, mostrar,
//! programar, pendientes/cancelar, el badge y la pulsación como evento `"notification"` con
//! `event_tap` (el id a secas o `id\taction\ttext\tdata`). Los backends reales (macOS `.app`, iOS,
//! Android) se verifican a mano; aquí se fija el contrato y la validación, que es común.

use std::process::Command;

const PROG: &str = r##"import std/ui;
import std/notify;

fn main() {
    print(notify.status());
    print(notify.request_permission());
    print(notify.request_permission_with(true, false));
    print(notify.create_channel(notify.Channel { id: "orders", name: "Pedidos", description: "", importance: "high", sound: "" }));
    print(notify.create_channel(notify.Channel { id: "x", name: "X", description: "", importance: "loud", sound: "" }));
    var n = notify.message("Pedido enviado", "Llega el viernes");
    n.id = "order-42";
    n.channel = "orders";
    n.group = "orders";
    n.data = "{\"order\":42}";
    n.badge = 3;
    n.interruption = "time_sensitive";
    n.actions = [notify.action("track", "Seguir"), notify.action("snooze", "Luego")];
    print(notify.show(n));
    print(notify.schedule(n, notify.Trigger.After(3600, false)));
    var d = notify.message("Diario", "");
    d.id = "daily";
    print(notify.schedule(d, notify.Trigger.Daily(9, 30)));
    print(notify.schedule(d, notify.Trigger.At(1800000000)));
    print(notify.schedule(notify.message("Pronto", ""), notify.Trigger.After(30, true)));
    print(notify.show(notify.message("", "")));
    n.interruption = "loud";
    print(notify.show(n));
    print(notify.pending());
    print(notify.cancel("order-42"));
    print(notify.pending());
    print(notify.cancel_all());
    print(notify.pending());
    print(notify.clear_delivered());
    print(notify.set_badge(0));
    print(ui.notify_with("Hola", "Mundo", "legacy", false));
    var k = 0;
    while (k < 4) {
        match (ui.next_event_timeout(500)) {
            Result.Ok(Option.Some(e)) => {
                print(e.kind + " [" + e.tag.replace("\t", "|") + "]");
                match (notify.event_tap(e)) {
                    Option.Some(t) => print(t.id + " / " + t.action + " / " + t.data),
                    Option.None => print("no tap"),
                }
                k = k + 1;
            },
            _ => { print("timeout"); k = 4; },
        }
    }
    print(notify.event_tap(ui.UiEvent { kind: "closed", window: 0, tag: "" }));
}
"##;

const WANT: &str = r##"Result.Ok(granted)
Result.Ok(granted)
Result.Ok(provisional)
Result.Ok(0)
Result.Err(notify: unknown channel importance 'loud' (min | low | default | high))
Result.Ok(order-42)
Result.Ok(order-42)
Result.Ok(daily)
Result.Ok(daily)
Result.Err(notify: a repeating After needs at least 60 seconds)
Result.Err(notify: a notification needs a title or a body)
Result.Err(notify: unknown interruption level 'loud' (passive | active | time_sensitive | critical))
Result.Ok([order-42, daily])
Result.Ok(0)
Result.Ok([daily])
Result.Ok(0)
Result.Ok([])
Result.Ok(0)
Result.Ok(0)
Result.Ok(0)
notification [order-42|track||{"order":42}]
order-42 / track / {"order":42}
notification [order-42|track||{"order":42}]
order-42 / track / {"order":42}
notification [daily|track||]
daily / track / 
notification [daily|track||]
daily / track / 
Option.None
"##;

fn run(bin: &Command_, dir: &std::path::Path, click: &str, permission: &str) -> (String, String) {
    let mut c = bin();
    c.env("RAY_UI_BACKEND", "headless").env("RAY_UI_NOTIFY_CLICK", click).env("RAY_UI_TRACE", "1").current_dir(dir);
    if !permission.is_empty() {
        c.env("RAY_UI_NOTIFY_PERMISSION", permission);
    }
    let out = c.output().unwrap();
    (String::from_utf8_lossy(&out.stdout).into_owned(), String::from_utf8_lossy(&out.stderr).into_owned())
}

type Command_ = dyn Fn() -> Command;

#[test]
fn notifications_show_schedule_and_tap_headless_on_all_engines() {
    let dir = std::env::temp_dir().join(format!("ray_ui_notify_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("a.ray"), PROG).unwrap();
    let mut engines: Vec<(String, Box<Command_>)> = vec![
        ("vm".into(), Box::new(|| { let mut c = Command::new(env!("CARGO_BIN_EXE_ray")); c.args(["run", "a.ray"]); c })),
        ("interp".into(), Box::new(|| { let mut c = Command::new(env!("CARGO_BIN_EXE_ray")); c.args(["run", "--interp", "a.ray"]); c })),
    ];
    // Nativo: el shim `__ray_ui_window` enruta `notify_*` igual que la VM (las apps empaquetadas son nativas).
    if Command::new("rustc").arg("--version").output().map(|o| o.status.success()).unwrap_or(false) {
        let bin = dir.join(format!("prog_bin{}", std::env::consts::EXE_SUFFIX));
        let st = Command::new(env!("CARGO_BIN_EXE_ray")).args(["build", "a.ray", "--native", "-o", bin.to_str().unwrap()]).current_dir(&dir).output().unwrap();
        assert!(st.status.success(), "build --native: {}", String::from_utf8_lossy(&st.stderr));
        engines.push(("native".into(), Box::new(move || Command::new(&bin))));
    }
    for (name, bin) in &engines {
        // `RAY_UI_NOTIFY_CLICK=track` pulsa la acción `track` de cada notificación mostrada (las
        // programadas también «se pulsan»: el inyector no espera al disparador).
        let (out, err) = run(bin, &dir, "track", "");
        assert_eq!(out, WANT, "{name}: {err}");
        assert!(err.contains("[ui] notify show order-42 \"Pedido enviado\" \"Llega el viernes\" channel=orders group=\"orders\" interruption=time_sensitive badge=3 sound=true actions=[track=\"Seguir\",snooze=\"Luego\"] trigger=now"), "{name}\n{err}");
        assert!(err.contains("trigger=after:3600:0") && err.contains("trigger=daily:9:30") && err.contains("trigger=at:1800000000"), "{name}\n{err}");
        assert!(err.contains("[ui] notify permission provisional=true time_sensitive=false"), "{name}\n{err}");
        assert!(err.contains("[ui] notify cancel order-42") && err.contains("[ui] notify badge 0"), "{name}\n{err}");
        // Permiso denegado: `show`/`schedule` fallan con el motivo y `status` lo cuenta.
        let (out, err) = run(bin, &dir, "", "denied");
        assert!(out.starts_with("Result.Ok(denied)\nResult.Ok(denied)\nResult.Ok(denied)\n"), "{name}: {out}\n{err}");
        assert!(out.contains("Result.Err(notify: notifications are not authorized for this app)"), "{name}: {out}");
        assert!(out.contains("\nResult.Ok([])\nResult.Ok(0)\nResult.Ok([])\n"), "{name}: sin pendientes al denegar\n{out}");
    }
    let _ = std::fs::remove_dir_all(&dir);
}
