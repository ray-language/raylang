//! Prueba de la CESIÓN cooperativa de `udp_recv_from` (M20.11, solo VM). `udp_yield_demo.ray` lanza dos
//! fibras que esperan un datagrama en sockets UDP distintos a la vez. Con un recv bloqueante, el
//! scheduler de un solo hilo haría **deadlock** (la 2.ª fibra nunca correría → su puerto no se imprime y
//! el test se cuelga). Que el test reciba AMBOS puertos y luego AMBAS respuestas prueba que `recv_from`
//! cede la fibra al scheduler mientras espera. Solo VM (la concurrencia es de la VM).

use std::io::{BufRead, BufReader};
use std::net::UdpSocket;
use std::process::{Command, Stdio};
use std::time::Duration;

#[test]
fn udp_recv_cede_la_fibra() {
    let demo = format!("{}/examples/web/udp_yield_demo.ray", env!("CARGO_MANIFEST_DIR"));
    let mut child = Command::new(env!("CARGO_BIN_EXE_raylang"))
        .arg("--vm")
        .arg(&demo)
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("lanza udp_yield_demo");

    let mut lineas = BufReader::new(child.stdout.take().unwrap()).lines();

    // Las dos primeras líneas son "A <portA>" y "B <portB>" (la B solo aparece si la 2.ª fibra corrió,
    // o sea si la 1.ª cedió en su recv). Que lleguen ambas ya descarta el deadlock.
    let l_a = lineas.next().expect("línea A").expect("io");
    let l_b = lineas.next().expect("línea B").expect("io");
    let port_a: u16 = l_a.strip_prefix("A ").expect("prefijo A").parse().expect("puerto A");
    let port_b: u16 = l_b.strip_prefix("B ").expect("prefijo B").parse().expect("puerto B");

    // Envía un datagrama a cada socket → ambas fibras despiertan.
    let s = UdpSocket::bind("127.0.0.1:0").expect("bind cliente");
    s.set_read_timeout(Some(Duration::from_secs(5))).ok();
    s.send_to(b"ping-a", ("127.0.0.1", port_a)).expect("send a");
    s.send_to(b"ping-b", ("127.0.0.1", port_b)).expect("send b");

    // Las dos líneas restantes (en cualquier orden) son los ecos.
    let mut resto = vec![
        lineas.next().expect("línea eco 1").expect("io"),
        lineas.next().expect("línea eco 2").expect("io"),
    ];
    resto.sort();
    assert_eq!(resto, vec!["A:ping-a".to_string(), "B:ping-b".to_string()]);

    let _ = child.kill();
    let _ = child.wait();
}
