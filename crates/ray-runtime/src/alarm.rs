//! M448 (IDEAS §122, pepa #341) — `std/alarm`: alarmas del SISTEMA, que suenan en bucle hasta que
//! el usuario responde aunque la app esté cerrada. No es una notificación con otro sonido: en
//! Android es `AlarmManager.setAlarmClock` + un servicio en primer plano que reproduce el sonido
//! con el volumen de alarma + una Activity a pantalla completa sobre el bloqueo (todo en el
//! shell, `RayAlarm.java`); en iOS 26+ es AlarmKit (`RayAlarm.swift` en el shell, M448b: la
//! alerta la pinta y la hace sonar el sistema). Aquí vive el contrato:
//! el formato del túnel `__ui_window(0, "alarm_<op>", arg)`, la validación, el backend headless
//! (traza + pendientes en memoria + respuesta inyectada) y el reparto por plataforma.
//!
//! La respuesta del usuario («Detener», «Posponer») llega al programa como un evento `"alarm"`
//! de la cola de `std/ui`, con `tag` = `id\taction\tdata`.

use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::Mutex;

pub const SEP: char = '\u{1f}';

/// Una alarma, ya validada.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Alarm {
    pub id: String,
    pub title: String,
    /// Un archivo de `[android] sounds` / `[ios] sounds`; vacío = el tono de alarma del sistema.
    pub sound: String,
    /// 0 = sin «Posponer».
    pub snooze_minutes: i64,
    /// Vacío = "Stop".
    pub stop_label: String,
    /// Vacío = "Snooze".
    pub snooze_label: String,
    pub data: String,
    /// Minutos máximos sonando sin respuesta (0 = el tope del shell, 60); después, `timeout`.
    pub max_minutes: i64,
}

/// Cuándo suena. Los días de `Weekly` son ISO: 1 = lunes … 7 = domingo.
#[derive(Debug, Clone, PartialEq)]
pub enum Trigger {
    At(i64),
    Daily { hour: i64, minute: i64 },
    Weekly { days: Vec<i64>, hour: i64, minute: i64 },
}

static SEQ: AtomicI64 = AtomicI64::new(0);

/// Un id para una alarma que no trae ninguno. Lleva el instante (ms), no el pid: las alarmas
/// persisten entre arranques y un pid se repite tras reiniciar — el id nuevo pisaría una armada.
fn next_id() -> String {
    let ms = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis()).unwrap_or(0);
    format!("alarm-{ms}-{}", SEQ.fetch_add(1, Ordering::SeqCst) + 1)
}

/// `at:<unix>` | `daily:<h>:<m>` | `weekly:<d,d,…>:<h>:<m>`.
pub fn parse_trigger(s: &str) -> Result<Trigger, String> {
    let mut it = s.split(':');
    let kind = it.next().unwrap_or("");
    let num = |v: Option<&str>, what: &str| -> Result<i64, String> {
        v.unwrap_or("").trim().parse::<i64>().map_err(|_| format!("alarm: bad trigger '{s}' ({what} must be an integer)"))
    };
    let time_of_day = |hour: i64, minute: i64, name: &str| -> Result<(), String> {
        if (0..24).contains(&hour) && (0..60).contains(&minute) {
            Ok(())
        } else {
            Err(format!("alarm: {name}({hour}, {minute}) is not a valid time of day"))
        }
    };
    match kind {
        "at" => {
            let unix = num(it.next(), "the time")?;
            let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0);
            if unix <= now {
                return Err("alarm: At(…) is in the past".to_string());
            }
            Ok(Trigger::At(unix))
        }
        "daily" => {
            let hour = num(it.next(), "the hour")?;
            let minute = num(it.next(), "the minute")?;
            time_of_day(hour, minute, "Daily")?;
            Ok(Trigger::Daily { hour, minute })
        }
        "weekly" => {
            let mut days: Vec<i64> = Vec::new();
            for d in it.next().unwrap_or("").split(',').filter(|d| !d.trim().is_empty()) {
                let d: i64 = d.trim().parse().map_err(|_| format!("alarm: bad trigger '{s}' (a weekday must be an integer)"))?;
                if !(1..=7).contains(&d) {
                    return Err(format!("alarm: weekday {d} is out of range (1 = Monday … 7 = Sunday)"));
                }
                if !days.contains(&d) {
                    days.push(d);
                }
            }
            if days.is_empty() {
                return Err("alarm: Weekly needs at least one weekday (1 = Monday … 7 = Sunday)".to_string());
            }
            days.sort_unstable();
            let hour = num(it.next(), "the hour")?;
            let minute = num(it.next(), "the minute")?;
            time_of_day(hour, minute, "Weekly")?;
            Ok(Trigger::Weekly { days, hour, minute })
        }
        other => Err(format!("alarm: unknown trigger '{other}'")),
    }
}

/// La forma en que viaja un `Trigger` (la inversa de [`parse_trigger`]).
pub fn trigger_text(t: &Trigger) -> String {
    match t {
        Trigger::At(unix) => format!("at:{unix}"),
        Trigger::Daily { hour, minute } => format!("daily:{hour}:{minute}"),
        Trigger::Weekly { days, hour, minute } => {
            format!("weekly:{}:{hour}:{minute}", days.iter().map(|d| d.to_string()).collect::<Vec<_>>().join(","))
        }
    }
}

/// `trigger ␟ id ␟ title ␟ sound ␟ snooze_minutes ␟ stop_label ␟ data ␟ max_minutes [␟ snooze_label]`.
/// Valida y rellena el id si falta.
pub fn parse_alarm(arg: &str) -> Result<(Trigger, Alarm), String> {
    let f: Vec<&str> = arg.split(SEP).collect();
    if f.len() < 8 {
        return Err("alarm: malformed alarm (regenerate std/alarm callers)".to_string());
    }
    let trigger = parse_trigger(f[0])?;
    let mut a = Alarm {
        id: f[1].trim().to_string(),
        title: f[2].to_string(),
        sound: f[3].trim().to_string(),
        snooze_minutes: f[4].trim().parse().unwrap_or(0),
        stop_label: f[5].to_string(),
        data: f[6].to_string(),
        max_minutes: f[7].trim().parse().unwrap_or(0),
        snooze_label: f.get(8).map(|s| s.to_string()).unwrap_or_default(),
    };
    if a.title.trim().is_empty() {
        return Err("alarm: an alarm needs a title".to_string());
    }
    if a.id.is_empty() {
        a.id = next_id();
    }
    // pepa #347: los mismos caracteres que un id de std/notify (el evento es `id\taction\tdata`);
    // el separador de campos del cable tampoco cabe.
    if a.id.contains(['\t', '\n', '\u{1f}']) {
        return Err("alarm: the id cannot contain tabs or newlines".to_string());
    }
    if a.sound.contains(['/', '\\']) {
        return Err("alarm: sound is a file name (from [android] sounds / [ios] sounds), not a path".to_string());
    }
    if !(0..=120).contains(&a.snooze_minutes) {
        return Err("alarm: snooze_minutes must be between 0 (no snooze) and 120".to_string());
    }
    if !(0..=60).contains(&a.max_minutes) {
        return Err("alarm: max_minutes must be between 0 (the default cap, 60) and 60".to_string());
    }
    // El separador de campos no puede viajar dentro de uno (desplazaría los demás).
    if arg.matches(SEP).count() > 8 || f.len() > 9 {
        return Err("alarm: a field contains the unit separator U+001F".to_string());
    }
    Ok((trigger, a))
}

/// El texto que viaja al shell (la inversa de [`parse_alarm`]).
pub fn alarm_text(trigger: &Trigger, a: &Alarm) -> String {
    [
        trigger_text(trigger),
        a.id.clone(),
        a.title.clone(),
        a.sound.clone(),
        a.snooze_minutes.to_string(),
        a.stop_label.clone(),
        a.data.clone(),
        a.max_minutes.to_string(),
        a.snooze_label.clone(),
    ]
    .join(&SEP.to_string())
}

/// El tag del evento `"alarm"`: `id\taction\tdata` (`action` = `stop` | `snooze` | `open`).
pub fn response_tag(id: &str, action: &str, data: &str) -> String {
    format!("{id}\t{action}\t{data}")
}

/// La entrada del túnel: `alarm_permission` | `alarm_status` | `alarm_schedule` | `alarm_cancel` |
/// `alarm_cancel_all` | `alarm_pending`.
pub fn op(op: &str, arg: &str) -> Result<String, String> {
    if crate::ui::is_headless() {
        return headless(op, arg);
    }
    match op {
        "alarm_permission" => backend::permission(),
        "alarm_status" => backend::status(),
        "alarm_schedule" => {
            let (trigger, alarm) = parse_alarm(arg)?;
            backend::schedule(trigger, alarm)
        }
        "alarm_cancel" => backend::cancel(arg.trim()).map(|_| String::new()),
        "alarm_cancel_all" => backend::cancel_all().map(|_| String::new()),
        "alarm_pending" => backend::pending().map(|ids| ids.join("\n")),
        other => Err(format!("alarm: unknown op '{other}'")),
    }
}

// ── Headless: todo en memoria, con traza. `RAY_UI_ALARM_PERMISSION` = `granted` (por defecto) |
// `denied` | `not_determined` | `unsupported`; `RAY_UI_ALARM_RESPONSE=stop|snooze` responde a
// cada alarma en cuanto se programa (tests). ──
fn headless(op: &str, arg: &str) -> Result<String, String> {
    fn pending() -> &'static Mutex<Vec<String>> {
        static P: Mutex<Vec<String>> = Mutex::new(Vec::new());
        &P
    }
    let trace = |line: &str| {
        if crate::ui::ui_trace() {
            eprintln!("[ui] alarm {line}");
        }
    };
    let status = || match std::env::var("RAY_UI_ALARM_PERMISSION").as_deref() {
        Ok("denied") => "denied",
        Ok("not_determined") => "not_determined",
        Ok("unsupported") => "unsupported",
        _ => "granted",
    };
    match op {
        "alarm_permission" => {
            trace("permission");
            match status() {
                "unsupported" => Err(UNSUPPORTED.to_string()),
                s => Ok(s.to_string()),
            }
        }
        "alarm_status" => Ok(status().to_string()),
        "alarm_schedule" => {
            let (trigger, a) = parse_alarm(arg)?;
            trace(&format!(
                "schedule {} {:?} sound={:?} snooze={} stop={:?} max={} trigger={}",
                a.id,
                a.title,
                a.sound,
                a.snooze_minutes,
                a.stop_label,
                a.max_minutes,
                trigger_text(&trigger)
            ));
            // Como el shell de Android: sin el permiso concedido no se arma (no habría botones).
            match status() {
                "unsupported" => return Err(UNSUPPORTED.to_string()),
                "granted" => {}
                st => return Err(format!("alarm: alarms are not authorized for this app (notifications: {st}; call request_permission)")),
            }
            {
                let mut p = pending().lock().unwrap();
                p.retain(|id| *id != a.id);
                p.push(a.id.clone());
            }
            if let Ok(action) = std::env::var("RAY_UI_ALARM_RESPONSE")
                && !action.is_empty()
            {
                crate::ui::push_alarm_event(&response_tag(&a.id, &action, &a.data));
                // Una alarma de una vez ya sonó; las que repiten siguen pendientes.
                if matches!(trigger, Trigger::At(_)) && action != "snooze" {
                    pending().lock().unwrap().retain(|id| *id != a.id);
                }
            }
            Ok(a.id)
        }
        "alarm_cancel" => {
            trace(&format!("cancel {}", arg.trim()));
            pending().lock().unwrap().retain(|id| id != arg.trim());
            Ok(String::new())
        }
        "alarm_cancel_all" => {
            trace("cancel all");
            pending().lock().unwrap().clear();
            Ok(String::new())
        }
        "alarm_pending" => Ok(pending().lock().unwrap().join("\n")),
        other => Err(format!("alarm: unknown op '{other}'")),
    }
}

/// Lo que responde una plataforma sin alarmas de sistema.
pub const UNSUPPORTED: &str = "alarm: system alarms are not available on this platform (they need the Android shell of `ray bundle --android`, or iOS 26+ with the shell of `ray bundle --ios`) — std/notify with a custom sound rings once";

// ── Android e iOS: el shell (RayAlarm.java / RayAlarm.swift sobre AlarmKit, iOS 26) por la
// petición genérica de M442. Un shell de iOS sin la pieza de alarmas (el programa no importaba
// std/alarm al empaquetar, o es anterior a 1.27.94) no registra el atendedor: `unsupported`. ──
#[cfg(any(target_os = "android", target_os = "ios"))]
mod backend {
    use super::{Alarm, Trigger};

    fn ask(op: &str, arg: &str) -> Result<String, String> {
        crate::ui::shell_request(op, arg).map_err(|e| {
            if cfg!(target_os = "ios") && e.contains("predates") {
                "alarm: this iOS shell has no alarm support (bundle a program that imports std/alarm with raylang 1.27.94 or newer)".to_string()
            } else {
                e
            }
        })
    }
    pub(super) fn permission() -> Result<String, String> {
        ask("alarm_permission", "")
    }
    pub(super) fn status() -> Result<String, String> {
        if cfg!(target_os = "ios") {
            return Ok(ask("alarm_status", "").unwrap_or_else(|_| "unsupported".to_string()));
        }
        ask("alarm_status", "")
    }
    pub(super) fn schedule(trigger: Trigger, a: Alarm) -> Result<String, String> {
        ask("alarm_schedule", &super::alarm_text(&trigger, &a)).map(|_| a.id)
    }
    /// Sin alarmas de sistema (un shell de iOS sin la pieza de alarmas) no hay nada que cancelar
    /// ni que listar: `Ok`, como en el escritorio y como documenta `cancel`.
    fn ask_quiet(op: &str, arg: &str) -> Result<String, String> {
        ask(op, arg).or_else(|e| if status().as_deref() == Ok("unsupported") { Ok(String::new()) } else { Err(e) })
    }
    pub(super) fn cancel(id: &str) -> Result<(), String> {
        ask_quiet("alarm_cancel", id).map(|_| ())
    }
    pub(super) fn cancel_all() -> Result<(), String> {
        ask_quiet("alarm_cancel_all", "").map(|_| ())
    }
    pub(super) fn pending() -> Result<Vec<String>, String> {
        ask_quiet("alarm_pending", "").map(|t| t.split('\n').filter(|s| !s.is_empty()).map(String::from).collect())
    }
}

// ── El escritorio: no hay alarmas de sistema. ──
#[cfg(not(any(target_os = "android", target_os = "ios")))]
mod backend {
    use super::{Alarm, Trigger, UNSUPPORTED};

    pub(super) fn permission() -> Result<String, String> {
        Err(UNSUPPORTED.to_string())
    }
    pub(super) fn status() -> Result<String, String> {
        Ok("unsupported".to_string())
    }
    pub(super) fn schedule(_trigger: Trigger, _a: Alarm) -> Result<String, String> {
        Err(UNSUPPORTED.to_string())
    }
    pub(super) fn cancel(_id: &str) -> Result<(), String> {
        Ok(())
    }
    pub(super) fn cancel_all() -> Result<(), String> {
        Ok(())
    }
    pub(super) fn pending() -> Result<Vec<String>, String> {
        Ok(Vec::new())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn enc(f: &[&str]) -> String {
        f.join(&SEP.to_string())
    }

    #[test]
    fn triggers_round_trip_and_validate() {
        assert_eq!(parse_trigger("daily:8:5").unwrap(), Trigger::Daily { hour: 8, minute: 5 });
        let w = parse_trigger("weekly:5,1,3,1:20:30").unwrap();
        assert_eq!(w, Trigger::Weekly { days: vec![1, 3, 5], hour: 20, minute: 30 });
        assert_eq!(trigger_text(&w), "weekly:1,3,5:20:30");
        assert_eq!(trigger_text(&parse_trigger("at:4102444800").unwrap()), "at:4102444800");
        assert_eq!(parse_trigger("at:5").unwrap_err(), "alarm: At(…) is in the past");
        assert!(parse_trigger("daily:24:0").unwrap_err().contains("not a valid time of day"));
        assert!(parse_trigger("weekly:8:9:0").unwrap_err().contains("out of range"));
        assert!(parse_trigger("weekly::9:0").unwrap_err().contains("at least one weekday"));
        assert!(parse_trigger("after:60").unwrap_err().contains("unknown trigger"));
    }

    #[test]
    fn an_alarm_is_parsed_validated_and_re_encoded() {
        let (t, a) = parse_alarm(&enc(&["daily:8:0", "med", "Hora de tu medicamento", "melody", "10", "Ya la tomé", "{\"dose\":1}", "5"])).unwrap();
        assert_eq!(t, Trigger::Daily { hour: 8, minute: 0 });
        assert_eq!((a.id.as_str(), a.sound.as_str(), a.snooze_minutes, a.max_minutes), ("med", "melody", 10, 5));
        assert_eq!(parse_alarm(&alarm_text(&t, &a)).unwrap(), (t, a));
        let (_, generated) = parse_alarm(&enc(&["daily:8:0", "", "T", "", "0", "", "", "0"])).unwrap();
        assert!(generated.id.starts_with("alarm-"));
        let bad = |f: &[&str]| parse_alarm(&enc(f)).unwrap_err();
        assert!(bad(&["daily:8:0", "x", " ", "", "0", "", "", "0"]).contains("needs a title"));
        assert!(bad(&["daily:8:0", "a\tb", "T", "", "0", "", "", "0"]).contains("tabs or newlines"));
        // pepa #347: un id con `|`, `#` o `:` vale, como en std/notify.
        let (_, piped) = parse_alarm(&enc(&["daily:8:0", "pepa:plan:2026-11-11|d|m1|20:00#ab", "T", "", "0", "", "", "0"])).unwrap();
        assert_eq!(piped.id, "pepa:plan:2026-11-11|d|m1|20:00#ab");
        assert!(bad(&["daily:8:0", "x", "T", "a/b.ogg", "0", "", "", "0"]).contains("not a path"));
        assert!(bad(&["daily:8:0", "x", "T", "", "500", "", "", "0"]).contains("snooze_minutes"));
        assert!(bad(&["daily:8:0", "x", "T", "", "0", "", "", "90"]).contains("max_minutes"));
        assert!(bad(&["daily:8:0", "x"]).contains("malformed"));
        // Un campo con el separador dentro desplazaría los demás: se rechaza.
        assert!(bad(&["daily:8:0", "x", "T\u{1f}oops", "", "0", "", "", "0", ""]).contains("unit separator"));
        assert!(bad(&["daily:8:0", "x", "T", "", "0", "", "", "61"]).contains("the default cap, 60"));
        assert_eq!(response_tag("med", "snooze", "d"), "med\tsnooze\td");
    }
}
