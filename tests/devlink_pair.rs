//! M330 D3: la página de emparejamiento de la librería de desarrollo — se sirve por `ray://app`
//! con el puente de siempre y devuelve la URL que la página manda con `window.ray.send`. En
//! headless, `RAY_UI_MSG` inyecta ese mensaje al abrir la ventana (el inyector de M152). Archivo
//! propio: la variable de entorno y la cola de eventos son del proceso.

#[test]
fn the_pairing_page_hands_back_the_link_the_page_sends() {
    ray_runtime::ui::default_headless();
    // SAFETY: proceso de test de un solo hilo en este punto (ningún otro test en este binario).
    unsafe { std::env::set_var("RAY_UI_MSG", "ray-dev://192.168.1.20:52731/abcdef") };
    let url = raylang::devlink::pair(None, "test-device");
    assert_eq!(url, "ray-dev://192.168.1.20:52731/abcdef");
    // La UI queda limpia para el programa: sin ventanas ni eventos pendientes.
    assert!(ray_runtime::ui::next_event_blocking(50).is_none());
}
