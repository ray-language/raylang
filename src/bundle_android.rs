//! M156 (§80b) — `ray bundle --android`: el proyecto GRADLE generado de una app Android. La
//! app es un SHELL delgado en Java (Activity + WebView a pantalla completa) que carga el
//! programa como cdylib (`System.loadLibrary("ray_app")` — `ray build --native --lib
//! --target aarch64-linux-android` por dentro): `RayBridge.start()` entra al `.so`
//! (`Java_org_raylang_shell_RayBridge_start` → registra handlers + `ray_start()`), y el
//! programa manda — su webserver embebido sirve la UI y `ui.open` entrega la URL al WebView.
//! MISMO FUENTE que el escritorio y que iOS.
//!
//! Decisiones de plantilla (plan M156): Java puro (sin toolchain kotlin), SIN
//! externalNativeBuild (todo lo nativo va dentro del .so — cero cmake/ninja), el paquete
//! Java es SIEMPRE `org.raylang.shell` (los símbolos JNI del .so son estables e
//! independientes del applicationId), cleartext SOLO para 127.0.0.1/localhost (network
//! security config, no el flag global), sin Gradle wrapper en v1 (el Gradle del sistema;
//! README documenta `gradle wrapper` para pinnear), `configChanges` en la Activity (la
//! rotación no recrea el WebView; si el sistema la mata igual, `RayBridge.lastUrl` recarga —
//! el espejo de la reconexión de escena de iOS).

use std::path::Path;

const SETTINGS_GRADLE: &str = r#"pluginManagement {
    repositories {
        google()
        mavenCentral()
        gradlePluginPortal()
    }
}
dependencyResolutionManagement {
    repositories {
        google()
        mavenCentral()
    }
}
include ':app'
"#;

const GRADLE_PROPERTIES: &str = r#"# Generado por `ray bundle --android`.
org.gradle.jvmargs=-Xmx2g
android.useAndroidX=true
"#;

/// El manifest del shell: INTERNET (el webserver embebido escucha en 127.0.0.1) + cleartext
/// acotado por la network security config. `configChanges`: la rotación no recrea la Activity.
const ANDROID_MANIFEST: &str = r#"<?xml version="1.0" encoding="utf-8"?>
<manifest xmlns:android="http://schemas.android.com/apk/res/android">
  <uses-permission android:name="android.permission.INTERNET" />
  <application
      android:label="@string/app_name"
      android:networkSecurityConfig="@xml/network_security_config"
      android:theme="@android:style/Theme.Material.Light.NoActionBar">
    <activity
        android:name=".MainActivity"
        android:exported="true"
        android:configChanges="orientation|screenSize|screenLayout|keyboardHidden">
      <intent-filter>
        <action android:name="android.intent.action.MAIN" />
        <category android:name="android.intent.category.LAUNCHER" />
      </intent-filter>
    </activity>
  </application>
</manifest>
"#;

/// Cleartext SOLO para el loopback (el patrón del proyecto: el webserver embebido en
/// 127.0.0.1) — jamás `usesCleartextTraffic` global.
const NETWORK_SECURITY_XML: &str = r#"<?xml version="1.0" encoding="utf-8"?>
<network-security-config>
  <domain-config cleartextTrafficPermitted="true">
    <domain includeSubdomains="false">127.0.0.1</domain>
    <domain includeSubdomains="false">localhost</domain>
  </domain-config>
</network-security-config>
"#;

/// La Activity: WebView a pantalla completa + el shim del puente IPC (M152 — `window.ray.send`
/// llega como evento "message" con window 0, como en el shell iOS) + eventos lifecycle.
const MAIN_ACTIVITY_JAVA: &str = r#"package org.raylang.shell;

import android.app.Activity;
import android.graphics.Bitmap;
import android.os.Bundle;
import android.webkit.JavascriptInterface;
import android.webkit.WebView;
import android.webkit.WebViewClient;

public class MainActivity extends Activity {
    @Override
    protected void onCreate(Bundle savedInstanceState) {
        super.onCreate(savedInstanceState);
        WebView web = new WebView(this);
        web.getSettings().setJavaScriptEnabled(true);
        web.getSettings().setDomStorageEnabled(true);
        web.addJavascriptInterface(new RayJs(), "RayAndroid");
        web.setWebViewClient(new WebViewClient() {
            @Override
            public void onPageStarted(WebView v, String url, Bitmap favicon) {
                // M152: el MISMO contrato que el user script de WKWebView — window.ray.send.
                v.evaluateJavascript(
                    "window.ray={send:function(t){RayAndroid.send(String(t).replace(/\\u0000/g,''))}}",
                    null);
            }
        });
        setContentView(web);
        RayBridge.attach(web);
        if (RayBridge.lastUrl != null) {
            web.loadUrl(RayBridge.lastUrl); // recreación: el programa sigue vivo, recargar
        }
        RayBridge.startOnce();
    }

    @Override
    protected void onPause() {
        super.onPause();
        RayBridge.pushEvent("lifecycle", 0, "background");
    }

    @Override
    protected void onResume() {
        super.onResume();
        RayBridge.pushEvent("lifecycle", 0, "foreground");
    }

    static final class RayJs {
        @JavascriptInterface
        public void send(String text) {
            RayBridge.pushEvent("message", 0, text == null ? "" : text);
        }
    }
}
"#;

/// El puente al `.so`: los natives resuelven contra los símbolos JNI emitidos DENTRO del
/// cdylib; onOpen/onEval llegan DESDE el hilo del programa raylang → Handler al main thread.
const RAY_BRIDGE_JAVA: &str = r#"package org.raylang.shell;

import android.os.Handler;
import android.os.Looper;
import android.webkit.WebView;

public final class RayBridge {
    static {
        System.loadLibrary("ray_app");
    }

    private static final Handler MAIN = new Handler(Looper.getMainLooper());
    private static WebView webView;
    static volatile String lastUrl;
    private static boolean started;

    static void attach(WebView w) {
        webView = w;
    }

    static void startOnce() {
        if (!started) {
            started = true;
            start(); // registra los handlers y lanza el programa raylang en su hilo
        }
    }

    public static native int start();

    public static native void pushEvent(String kind, long window, String tag);

    // Llamados desde NATIVO (el hilo del programa): siempre postear al main thread.
    public static void onOpen(String title, String url) {
        lastUrl = url;
        MAIN.post(() -> {
            if (webView != null) {
                webView.loadUrl(url);
            }
        });
    }

    public static void onEval(String js) {
        MAIN.post(() -> {
            if (webView != null) {
                webView.evaluateJavascript(js, null);
            }
        });
    }
}
"#;

/// build.gradle de la app: AGP pinneado, SIN externalNativeBuild (el .so viene hecho).
fn app_build_gradle(app_id: &str, version: &str, abis: &str) -> String {
    format!(
        r#"plugins {{
    id 'com.android.application' version '9.0.0' // compatible con Gradle 9.x (AGP 8.x usa una API interna retirada en 9.6)
}}

android {{
    namespace 'org.raylang.shell'
    compileSdk 35

    defaultConfig {{
        applicationId "{app_id}"
        minSdk 24
        targetSdk 35
        versionCode 1
        versionName "{version}"
        ndk {{
            abiFilters {abis}
        }}
    }}

    compileOptions {{
        sourceCompatibility JavaVersion.VERSION_17
        targetCompatibility JavaVersion.VERSION_17
    }}
}}
"#
    )
}

fn strings_xml(name: &str) -> String {
    format!(
        "<?xml version=\"1.0\" encoding=\"utf-8\"?>\n<resources>\n  <string name=\"app_name\">{name}</string>\n</resources>\n"
    )
}

fn settings_gradle(name: &str) -> String {
    // El bloque pluginManagement debe ser LO PRIMERO del script (regla de Gradle).
    format!("{SETTINGS_GRADLE}rootProject.name = \"{name}\"\n")
}

const README: &str = r#"# App Android generada por `ray bundle --android`

- `app/src/main/jniLibs/<abi>/libray_app.so` es el programa raylang compilado como cdylib;
  para regenerarlo tras cambiar el programa: `ray bundle --android` de nuevo
  (`--android-abi arm64|x86_64|all` construye solo un ABI; el otro `.so` se conserva).
- Compilar el APK (Gradle del sistema + JDK 17+; `gradle wrapper` si quieres pinnear):
  `gradle assembleDebug` → `app/build/outputs/apk/debug/app-debug.apk`.
- Emulador/dispositivo: `adb install -r app/build/outputs/apk/debug/app-debug.apk` y lanza la
  app (o `adb shell am start -n <applicationId>/org.raylang.shell.MainActivity`). OJO: un APK
  solo-arm64 no instala en un emulador x86_64 (INSTALL_FAILED_NO_MATCHING_ABIS) — usa
  `--android-abi all`.
- stdout/stderr del programa van a **logcat** con tag `ray`: `adb logcat -s ray`.
- El puente IPC (M152) funciona igual que en escritorio/iOS: `window.ray.send(text)` llega
  como evento `"message"` (window 0). Los eventos `lifecycle` llegan en onPause/onResume.
- `std/fs`/`std/kv`: escribe en el directorio privado de la app (el cwd no es tuyo); las
  rutas externas están restringidas (scoped storage) — también para `fs.watch`.
- Firma: el debug keystore de Gradle basta para instalar; firma de release = v2 (keystore
  propio, fuera del v1).
"#;

/// Genera el árbol del proyecto en `dir` (ya creado). Los `.so` los copia el llamador a
/// `app/src/main/jniLibs/<abi>/`; `local.properties` lo escribe el llamador SOLO si no existe.
pub fn write_project(dir: &Path, name: &str, app_id: &str, version: &str, abis: &str) -> Result<(), String> {
    let write = |rel: &str, content: &str| -> Result<(), String> {
        let p = dir.join(rel);
        if let Some(parent) = p.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        std::fs::write(&p, content).map_err(|e| format!("{}: {e}", p.display()))
    };
    write("settings.gradle", &settings_gradle(name))?;
    write("gradle.properties", GRADLE_PROPERTIES)?;
    write("app/build.gradle", &app_build_gradle(app_id, version, abis))?;
    write("app/src/main/AndroidManifest.xml", ANDROID_MANIFEST)?;
    write("app/src/main/res/xml/network_security_config.xml", NETWORK_SECURITY_XML)?;
    write("app/src/main/res/values/strings.xml", &strings_xml(name))?;
    write("app/src/main/java/org/raylang/shell/MainActivity.java", MAIN_ACTIVITY_JAVA)?;
    write("app/src/main/java/org/raylang/shell/RayBridge.java", RAY_BRIDGE_JAVA)?;
    write("README.md", README)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_templates_carry_the_load_bearing_pieces() {
        // Los invariantes del contrato: loadLibrary del nombre FIJO, los natives que casan
        // con los símbolos JNI emitidos, el shim M152, y el cleartext ACOTADO al loopback.
        assert!(RAY_BRIDGE_JAVA.contains("System.loadLibrary(\"ray_app\")"));
        assert!(RAY_BRIDGE_JAVA.contains("public static native int start()"));
        assert!(RAY_BRIDGE_JAVA
            .contains("public static native void pushEvent(String kind, long window, String tag)"));
        assert!(MAIN_ACTIVITY_JAVA.contains("window.ray={send:function(t){RayAndroid.send("));
        assert!(ANDROID_MANIFEST
            .contains("android:networkSecurityConfig=\"@xml/network_security_config\""));
        assert!(NETWORK_SECURITY_XML.contains("127.0.0.1"));
        assert!(!ANDROID_MANIFEST.contains("usesCleartextTraffic"));
        let gradle = app_build_gradle("org.raylang.demo", "1.0.0", "'arm64-v8a'");
        assert!(gradle.contains("namespace 'org.raylang.shell'"), "{gradle}");
        assert!(gradle.contains("applicationId \"org.raylang.demo\""), "{gradle}");
        assert!(gradle.contains("abiFilters 'arm64-v8a'"), "{gradle}");
        assert!(!gradle.contains("externalNativeBuild"), "todo lo nativo va dentro del .so");
    }
}
