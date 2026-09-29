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
/// M160: `android:icon` SOLO cuando los PNG multi-densidad se generaron de verdad — el
/// atributo con los mipmaps ausentes rompe el build en aapt (generar-primero-decidir-después).
fn android_manifest(icon: bool, background_audio: bool) -> String {
    let icon_attr = if icon { "\n      android:icon=\"@mipmap/ic_launcher\"" } else { "" };
    // M324 (ray808 #18): el foreground service de reproducción y sus permisos (FOREGROUND_SERVICE
    // desde API 28, su tipo mediaPlayback desde API 34, la notificación desde API 33).
    let (audio_perms, audio_service) = if background_audio {
        (
            "\n  <uses-permission android:name=\"android.permission.FOREGROUND_SERVICE\" />\n  <uses-permission android:name=\"android.permission.FOREGROUND_SERVICE_MEDIA_PLAYBACK\" />\n  <uses-permission android:name=\"android.permission.POST_NOTIFICATIONS\" />",
            "\n    <service\n        android:name=\".RayPlaybackService\"\n        android:exported=\"false\"\n        android:foregroundServiceType=\"mediaPlayback\" />",
        )
    } else {
        ("", "")
    };
    format!(
        r#"<?xml version="1.0" encoding="utf-8"?>
<manifest xmlns:android="http://schemas.android.com/apk/res/android">
  <uses-permission android:name="android.permission.INTERNET" />{audio_perms}
  <application
      android:label="@string/app_name"{icon_attr}
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
    </activity>{audio_service}
  </application>
</manifest>
"#
    )
}

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
import android.content.Intent;
import android.graphics.Bitmap;
import android.net.Uri;
import android.os.Build;
import android.os.Bundle;
import android.webkit.JavascriptInterface;
import android.webkit.ValueCallback;
import android.webkit.WebChromeClient;
import android.webkit.WebResourceRequest;
import android.webkit.WebResourceResponse;
import android.webkit.WebView;
import android.webkit.WebViewClient;
import androidx.webkit.WebViewCompat;
import androidx.webkit.WebViewFeature;
import java.util.Collections;

public class MainActivity extends Activity {
    // M152: el puente IPC — el MISMO contrato que el user script de WKWebView (window.ray.send /
    // request / _deliver / _deliver_json).
    static final String RAY_SHIM =
        "(function(){var p={},n=0;function e(t){return typeof t==='string'?t:JSON.stringify(t)}"
            + "function q(s){RayAndroid.send(String(s).replace(/\\u0000/g,''))}"
            + "window.ray={send:function(t){q(e(t))},request:function(t){n=n+1;var i=n;"
            + "return new Promise(function(r){p[i]=r;q('\\u0001q\\u0001'+i+'\\u0001'+e(t))})},"
            + "_deliver:function(i,v){var r=p[i];if(r){delete p[i];r(v)}},"
            + "_deliver_json:function(i,t){var r=p[i];if(r){delete p[i];r(JSON.parse(t))}}}})()";

    // M309 (findings #41): el <input type="file"> de la página abre el selector del sistema.
    private static final int RAY_FILE_CHOOSER = 7001;
    private ValueCallback<Uri[]> rayFileCallback = null;
    // M324 (ray808 #18): `[android] background_audio` — foreground service mientras la app está
    // en segundo plano, para que el sistema no mate el proceso (y con él std/audio).
    static final boolean RAY_BACKGROUND_AUDIO = /*RAY_BACKGROUND_AUDIO*/false;
    private static final int RAY_NOTIFICATIONS = 7002;

    @Override
    protected void onCreate(Bundle savedInstanceState) {
        super.onCreate(savedInstanceState);
        // M309 (findings #40): el programa corre con cwd=/ y sin HOME — no tendría dónde escribir.
        // HOME = el directorio de datos privado de la app; TMPDIR = su caché (fs.temp_dir()).
        try {
            android.system.Os.setenv("HOME", getFilesDir().getAbsolutePath(), true);
            android.system.Os.setenv("TMPDIR", getCacheDir().getAbsolutePath(), true);
        } catch (Exception e) { }
        WebView web = new WebView(this);
        web.getSettings().setJavaScriptEnabled(true);
        web.getSettings().setDomStorageEnabled(true);
        web.getSettings().setAllowFileAccess(true);
        /*RAY_DEVTOOLS*/
        web.setWebChromeClient(new WebChromeClient() {
            @Override
            public boolean onShowFileChooser(WebView v, ValueCallback<Uri[]> cb, FileChooserParams params) {
                if (rayFileCallback != null) { rayFileCallback.onReceiveValue(null); }
                rayFileCallback = cb;
                Intent intent = params.createIntent();
                try {
                    startActivityForResult(intent, RAY_FILE_CHOOSER);
                } catch (Exception e) {
                    rayFileCallback = null;
                    return false;
                }
                return true;
            }
        });
        web.addJavascriptInterface(new RayJs(), "RayAndroid");
        // M323 (ray808 #12): el shim va como script de INICIO DE DOCUMENTO, antes de cualquier
        // <script> de la página — una página servida por ray://app ejecuta el suyo antes de que
        // llegue onPageStarted. Si el WebView del dispositivo no lo soporta, onPageStarted como antes.
        boolean shimAtStart = false;
        if (WebViewFeature.isFeatureSupported(WebViewFeature.DOCUMENT_START_SCRIPT)) {
            WebViewCompat.addDocumentStartJavaScript(web, RAY_SHIM, Collections.singleton("*"));
            shimAtStart = true;
        }
        final boolean shimInjected = shimAtStart;
        web.setWebViewClient(new WebViewClient() {
            @Override
            public WebResourceResponse shouldInterceptRequest(WebView v, WebResourceRequest req) {
                // M322: ray://app/… (y su alias https) se sirve desde el programa.
                return RayScheme.intercept(req);
            }

            @Override
            public boolean shouldOverrideUrlLoading(WebView v, WebResourceRequest req) {
                // Un enlace ray://app/… navega por el alias https (origen y fetch() válidos).
                String u = req.getUrl().toString();
                if (u.startsWith("ray://app")) {
                    v.loadUrl(RayScheme.alias(u));
                    return true;
                }
                return false;
            }

            @Override
            public void onPageStarted(WebView v, String url, Bitmap favicon) {
                if (!shimInjected) {
                    v.evaluateJavascript(RAY_SHIM, null); // WebView sin DOCUMENT_START_SCRIPT
                }
            }
        });
        setContentView(web);
        if (RAY_BACKGROUND_AUDIO && Build.VERSION.SDK_INT >= 33
                && checkSelfPermission("android.permission.POST_NOTIFICATIONS") != android.content.pm.PackageManager.PERMISSION_GRANTED) {
            requestPermissions(new String[] { "android.permission.POST_NOTIFICATIONS" }, RAY_NOTIFICATIONS);
        }
        RayBridge.attach(web);
        if (RayBridge.lastUrl != null) {
            web.loadUrl(RayBridge.lastUrl); // recreación: el programa sigue vivo, recargar
        }
        RayBridge.startOnce();
    }

    @Override
    protected void onActivityResult(int requestCode, int resultCode, Intent data) {
        if (requestCode == RAY_FILE_CHOOSER) {
            if (rayFileCallback != null) {
                rayFileCallback.onReceiveValue(WebChromeClient.FileChooserParams.parseResult(resultCode, data));
                rayFileCallback = null;
            }
            return;
        }
        super.onActivityResult(requestCode, resultCode, data);
    }

    @Override
    protected void onPause() {
        super.onPause();
        if (RAY_BACKGROUND_AUDIO) { RayPlaybackService.start(this); }
        RayBridge.pushEvent("lifecycle", 0, "background");
    }

    @Override
    protected void onResume() {
        super.onResume();
        if (RAY_BACKGROUND_AUDIO) { RayPlaybackService.stop(this); }
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

/// M324 (ray808 #18): el foreground service de reproducción. Vive SOLO mientras la app está en
/// segundo plano (MainActivity lo arranca en onPause y lo para en onResume): la notificación
/// «playing» es el precio que Android pide por no matar el proceso. Tocarla vuelve a la app.
const RAY_PLAYBACK_SERVICE_JAVA: &str = r#"package org.raylang.shell;

import android.app.Notification;
import android.app.NotificationChannel;
import android.app.NotificationManager;
import android.app.PendingIntent;
import android.app.Service;
import android.content.Context;
import android.content.Intent;
import android.content.pm.ServiceInfo;
import android.os.Build;
import android.os.IBinder;

public class RayPlaybackService extends Service {
    static final String CHANNEL = "ray_playback";
    static final int NOTIFICATION_ID = 1;

    static void start(Context c) {
        Intent i = new Intent(c, RayPlaybackService.class);
        if (Build.VERSION.SDK_INT >= 26) { c.startForegroundService(i); } else { c.startService(i); }
    }

    static void stop(Context c) {
        c.stopService(new Intent(c, RayPlaybackService.class));
    }

    @Override
    public int onStartCommand(Intent intent, int flags, int startId) {
        CharSequence label = getApplicationInfo().loadLabel(getPackageManager());
        Notification.Builder b;
        if (Build.VERSION.SDK_INT >= 26) {
            NotificationManager nm = getSystemService(NotificationManager.class);
            nm.createNotificationChannel(new NotificationChannel(CHANNEL, label, NotificationManager.IMPORTANCE_LOW));
            b = new Notification.Builder(this, CHANNEL);
        } else {
            b = new Notification.Builder(this);
        }
        Intent open = new Intent(this, MainActivity.class);
        open.setFlags(Intent.FLAG_ACTIVITY_SINGLE_TOP);
        Notification n = b.setContentTitle(label)
            .setContentText("Playing in the background")
            .setSmallIcon(android.R.drawable.ic_media_play)
            .setContentIntent(PendingIntent.getActivity(this, 0, open, PendingIntent.FLAG_IMMUTABLE))
            .setOngoing(true)
            .build();
        if (Build.VERSION.SDK_INT >= 29) {
            startForeground(NOTIFICATION_ID, n, ServiceInfo.FOREGROUND_SERVICE_TYPE_MEDIA_PLAYBACK);
        } else {
            startForeground(NOTIFICATION_ID, n);
        }
        return START_NOT_STICKY;
    }

    @Override
    public void onDestroy() {
        stopForeground(Service.STOP_FOREGROUND_REMOVE);
        super.onDestroy();
    }

    @Override
    public IBinder onBind(Intent intent) {
        return null;
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
            capabilities(1); // M323: este shell sirve ray://app (RayScheme)
            start(); // registra los handlers y lanza el programa raylang en su hilo
        }
    }

    public static native void capabilities(int caps);

    public static native int start();

    public static native void pushEvent(String kind, long window, String tag);

    // M322: el esquema ray://app/… (ver RayScheme).
    public static native long schemeOpen(String url, String method, String range, String ifNoneMatch);

    public static native int schemeStatus(long handle);

    public static native String schemeHeaders(long handle);

    public static native byte[] schemeRead(long handle);

    public static native void schemeClose(long handle);

    // Llamados desde NATIVO (el hilo del programa): siempre postear al main thread.
    public static void onOpen(String title, String url) {
        String target = RayScheme.alias(url); // M322: ray://app/… carga por su alias https
        lastUrl = target;
        MAIN.post(() -> {
            if (webView != null) {
                webView.loadUrl(target);
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

/// M322: `ray://app/…` servido desde el programa. Chromium (el WebView) no permite `fetch()`/XHR
/// hacia un esquema propio ni le da un origen con localStorage, así que la página se carga por
/// el ALIAS `https://app.ray.invalid/…` (`.invalid` jamás resuelve: si el intercept faltara, no
/// hay sitio real detrás) y `shouldInterceptRequest` atiende las dos formas con el mismo
/// resolver que los shells de escritorio (montajes, Range, MIME). `If-None-Match` no se reenvía:
/// `WebResourceResponse` no admite un 304, así que cada petición se sirve completa.
const RAY_SCHEME_JAVA: &str = r#"package org.raylang.shell;

import android.net.Uri;
import android.webkit.WebResourceRequest;
import android.webkit.WebResourceResponse;
import java.io.InputStream;
import java.util.HashMap;
import java.util.Map;

final class RayScheme {
    static final String ALIAS_HOST = "app.ray.invalid";
    static final String ALIAS = "https://" + ALIAS_HOST;

    /** ray://app/x → https://app.ray.invalid/x; cualquier otra URL se devuelve tal cual. */
    static String alias(String url) {
        return url.startsWith("ray://app") ? ALIAS + url.substring("ray://app".length()) : url;
    }

    static WebResourceResponse intercept(WebResourceRequest req) {
        Uri u = req.getUrl();
        String scheme = u.getScheme();
        String host = u.getHost();
        boolean own = ("ray".equals(scheme) && "app".equals(host))
            || ("https".equals(scheme) && ALIAS_HOST.equals(host));
        if (!own) {
            return null;
        }
        String path = u.getEncodedPath();
        String url = "ray://app" + (path == null ? "" : path);
        String method = req.getMethod();
        Map<String, String> in = req.getRequestHeaders();
        String range = in == null ? null : in.get("Range");
        long handle = RayBridge.schemeOpen(url, method == null ? "GET" : method, range, null);
        if (handle == 0) {
            return null;
        }
        int status = RayBridge.schemeStatus(handle);
        String mime = "application/octet-stream";
        String encoding = null;
        Map<String, String> headers = new HashMap<>();
        String raw = RayBridge.schemeHeaders(handle);
        if (raw != null) {
            for (String line : raw.split("\n")) {
                int i = line.indexOf(": ");
                if (i < 0) {
                    continue;
                }
                String k = line.substring(0, i);
                String val = line.substring(i + 2);
                if (k.equalsIgnoreCase("Content-Type")) {
                    int semi = val.indexOf(';');
                    mime = (semi < 0 ? val : val.substring(0, semi)).trim();
                    int cs = val.toLowerCase().indexOf("charset=");
                    if (cs >= 0) {
                        encoding = val.substring(cs + "charset=".length()).trim();
                    }
                } else {
                    headers.put(k, val);
                }
            }
        }
        WebResourceResponse r = new WebResourceResponse(mime, encoding, new Body(handle));
        r.setStatusCodeAndReasonPhrase(status, reason(status));
        r.setResponseHeaders(headers);
        return r;
    }

    static String reason(int status) {
        switch (status) {
            case 200: return "OK";
            case 206: return "Partial Content";
            case 403: return "Forbidden";
            case 404: return "Not Found";
            case 405: return "Method Not Allowed";
            case 416: return "Range Not Satisfiable";
            default: return "Status " + status;
        }
    }

    /** El cuerpo, trozo a trozo desde el programa; cerrar libera el handle (también a medias). */
    static final class Body extends InputStream {
        private long handle;
        private byte[] chunk = null;
        private int pos = 0;

        Body(long handle) {
            this.handle = handle;
        }

        @Override
        public int read() {
            byte[] one = new byte[1];
            int n = read(one, 0, 1);
            return n <= 0 ? -1 : (one[0] & 0xff);
        }

        @Override
        public int read(byte[] b, int off, int len) {
            if (len == 0) {
                return 0;
            }
            if (chunk == null || pos >= chunk.length) {
                if (handle == 0) {
                    return -1;
                }
                chunk = RayBridge.schemeRead(handle);
                pos = 0;
                if (chunk == null || chunk.length == 0) {
                    close();
                    return -1;
                }
            }
            int n = Math.min(len, chunk.length - pos);
            System.arraycopy(chunk, pos, b, off, n);
            pos += n;
            return n;
        }

        @Override
        public void close() {
            if (handle != 0) {
                RayBridge.schemeClose(handle);
                handle = 0;
            }
        }
    }
}
"#;

/// build.gradle de la app: AGP pinneado, SIN externalNativeBuild (el .so viene hecho).
/// M160: firma de release CONDICIONAL a `keystore.properties` en la raíz del proyecto — cero
/// secretos en ray.toml ni en este archivo; sin el properties, el bloque no aplica y el debug
/// keystore de Gradle sigue mandando. `rootProject.file(...)` resuelve `storeFile` relativo a
/// la raíz (donde el README manda crear ambos; el bundle los preserva al regenerar).
fn app_build_gradle(app_id: &str, version: &str, abis: &str) -> String {
    format!(
        r#"plugins {{
    id 'com.android.application' version '9.0.0' // compatible con Gradle 9.x (AGP 8.x usa una API interna retirada en 9.6)
}}

def keystorePropsFile = rootProject.file('keystore.properties')
def keystoreProps = new Properties()
if (keystorePropsFile.exists()) {{
    keystorePropsFile.withInputStream {{ keystoreProps.load(it) }}
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

    signingConfigs {{
        if (keystorePropsFile.exists()) {{
            release {{
                storeFile rootProject.file(keystoreProps['storeFile'])
                storePassword keystoreProps['storePassword']
                keyAlias keystoreProps['keyAlias']
                keyPassword keystoreProps['keyPassword']
            }}
        }}
    }}

    buildTypes {{
        release {{
            if (keystorePropsFile.exists()) {{
                signingConfig signingConfigs.release
            }}
        }}
    }}

    compileOptions {{
        sourceCompatibility JavaVersion.VERSION_17
        targetCompatibility JavaVersion.VERSION_17
    }}
}}

dependencies {{
    // M323: WebViewCompat.addDocumentStartJavaScript (el shim window.ray antes de la página).
    implementation 'androidx.webkit:webkit:1.12.1'
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
- `ray://app/…` (M322): `ui.open` con una URL `ray://app/…` la sirve desde el programa
  (`ui.mount_embed`/`mount_dir`/`mount_bytes`), sin puerto. El WebView la carga por el alias
  `https://app.ray.invalid/…` (Chromium no admite `fetch()` hacia un esquema propio); las
  rutas relativas y `fetch("/api/x")` de la página funcionan igual, y un enlace absoluto
  `ray://app/…` se reescribe al alias al navegar.
- `std/fs`/`std/kv`: escribe en el directorio privado de la app (el cwd no es tuyo); las
  rutas externas están restringidas (scoped storage) — también para `fs.watch`.
- Firma: el debug keystore de Gradle basta para instalar. **Release** (M160): crea un
  keystore y un `keystore.properties` en ESTA raíz del proyecto —
  `keytool -genkeypair -v -keystore release.jks -alias app -keyalg RSA -keysize 2048 -validity 10000`
  y luego:

  ```
  storeFile=release.jks
  storePassword=...
  keyAlias=app
  keyPassword=...
  ```

  Con eso, `gradle assembleRelease` → `app/build/outputs/apk/release/app-release.apk`
  firmado. Ambos archivos se PRESERVAN al regenerar con `ray bundle --android`
  (`keystore.properties` y los `*.jks`/`*.keystore` de la raíz); no los subas al VCS.
- Icono (M160): `ray bundle --android --icon icon.png` genera los `mipmap-*/ic_launcher.png`
  multi-densidad (necesita `sips`, macOS). Es el icono legacy: en Android 8+ el sistema lo
  enmascara a círculo (el adaptive icon con capas queda para v2).
"#;

/// Genera el árbol del proyecto en `dir` (ya creado). Los `.so` los copia el llamador a
/// `app/src/main/jniLibs/<abi>/`; `local.properties` lo escribe el llamador SOLO si no existe.
/// M160: `icon` = true SOLO si el llamador ya generó los PNG (los copia él a `mipmap-*/`).
#[cfg(test)]
mod devtools_tests {
    /// M231: `--devtools` deja `setWebContentsDebuggingEnabled(true)`; sin el flag, nada.
    #[test]
    fn devtools_flag_toggles_remote_debugging_in_the_shell() {
        let base = std::env::temp_dir().join(format!("ray_android_devtools_{}", std::process::id()));
        for (devtools, want) in [(true, true), (false, false)] {
            let _ = std::fs::remove_dir_all(&base);
            super::write_project(&base, "App", "org.example.app", "1.0.0", "arm64-v8a", false, devtools, false).unwrap();
            let src = std::fs::read_to_string(base.join("app/src/main/java/org/raylang/shell/MainActivity.java")).unwrap();
            assert_eq!(src.contains("setWebContentsDebuggingEnabled(true)"), want, "devtools={devtools}");
            assert!(!src.contains("/*RAY_DEVTOOLS*/"), "el marcador no queda en el proyecto");
        }
        let _ = std::fs::remove_dir_all(&base);
    }
}

pub fn write_project(
    dir: &Path,
    name: &str,
    app_id: &str,
    version: &str,
    abis: &str,
    icon: bool,
    devtools: bool,
    background_audio: bool,
) -> Result<(), String> {
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
    write("app/src/main/AndroidManifest.xml", &android_manifest(icon, background_audio))?;
    write("app/src/main/res/xml/network_security_config.xml", NETWORK_SECURITY_XML)?;
    write("app/src/main/res/values/strings.xml", &strings_xml(name))?;
    // M231: `--devtools` → depuración remota desde chrome://inspect (Chrome del escritorio).
    let devtools_line = if devtools { "WebView.setWebContentsDebuggingEnabled(true); // ray bundle --devtools" } else { "" };
    // M324 (ray808 #18): el flag de audio en segundo plano va como constante de la clase.
    let bg = if background_audio { "true" } else { "false" };
    let main = MAIN_ACTIVITY_JAVA.replace("/*RAY_DEVTOOLS*/", devtools_line).replace("/*RAY_BACKGROUND_AUDIO*/false", bg);
    write("app/src/main/java/org/raylang/shell/MainActivity.java", &main)?;
    if background_audio {
        write("app/src/main/java/org/raylang/shell/RayPlaybackService.java", RAY_PLAYBACK_SERVICE_JAVA)?;
    }
    write("app/src/main/java/org/raylang/shell/RayBridge.java", RAY_BRIDGE_JAVA)?;
    write("app/src/main/java/org/raylang/shell/RayScheme.java", RAY_SCHEME_JAVA)?;
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
        assert!(MAIN_ACTIVITY_JAVA.contains("window.ray={send:function(t){q(e(t))}"));
        assert!(MAIN_ACTIVITY_JAVA.contains("request:function(t)"), "M157: request in the shim");
        // M323 (ray808 #12): el shim como script de inicio de documento, con fallback a onPageStarted.
        assert!(MAIN_ACTIVITY_JAVA.contains("WebViewCompat.addDocumentStartJavaScript(web, RAY_SHIM, Collections.singleton(\"*\"))"));
        assert!(MAIN_ACTIVITY_JAVA.contains("if (!shimInjected) {\n                    v.evaluateJavascript(RAY_SHIM, null);"));
        assert!(app_build_gradle("org.raylang.demo", "1.0.0", "'arm64-v8a'").contains("implementation 'androidx.webkit:webkit:1.12.1'"));
        assert!(android_manifest(false, false)
            .contains("android:networkSecurityConfig=\"@xml/network_security_config\""));
        assert!(NETWORK_SECURITY_XML.contains("127.0.0.1"));
        assert!(!android_manifest(false, false).contains("usesCleartextTraffic"));
        let gradle = app_build_gradle("org.raylang.demo", "1.0.0", "'arm64-v8a'");
        assert!(gradle.contains("namespace 'org.raylang.shell'"), "{gradle}");
        assert!(gradle.contains("applicationId \"org.raylang.demo\""), "{gradle}");
        assert!(gradle.contains("abiFilters 'arm64-v8a'"), "{gradle}");
        assert!(!gradle.contains("externalNativeBuild"), "todo lo nativo va dentro del .so");
    }

    /// M322: el shell intercepta `ray://app/…` y su alias https con los natives del esquema, y
    /// `onOpen` navega por el alias (fetch()/origen válidos en Chromium).
    #[test]
    fn the_shell_serves_the_ray_scheme_from_the_program() {
        for native in [
            "public static native long schemeOpen(String url, String method, String range, String ifNoneMatch)",
            "public static native int schemeStatus(long handle)",
            "public static native String schemeHeaders(long handle)",
            "public static native byte[] schemeRead(long handle)",
            "public static native void schemeClose(long handle)",
        ] {
            assert!(RAY_BRIDGE_JAVA.contains(native), "{native}");
        }
        assert!(RAY_BRIDGE_JAVA.contains("String target = RayScheme.alias(url);"));
        assert!(RAY_BRIDGE_JAVA.contains("capabilities(1); // M323"));
        assert!(RAY_BRIDGE_JAVA.contains("public static native void capabilities(int caps)"));
        assert!(MAIN_ACTIVITY_JAVA.contains("public WebResourceResponse shouldInterceptRequest(WebView v, WebResourceRequest req)"));
        assert!(MAIN_ACTIVITY_JAVA.contains("return RayScheme.intercept(req);"));
        assert!(RAY_SCHEME_JAVA.contains("static final String ALIAS_HOST = \"app.ray.invalid\";"));
        assert!(RAY_SCHEME_JAVA.contains("(\"ray\".equals(scheme) && \"app\".equals(host))"));
        // If-None-Match nunca viaja: WebResourceResponse no admite 304.
        assert!(RAY_SCHEME_JAVA.contains("RayBridge.schemeOpen(url, method == null ? \"GET\" : method, range, null)"));
        assert!(!android_manifest(false, false).contains("app.ray.invalid"), "the alias needs no manifest entry");
    }

    #[test]
    fn the_manifest_only_declares_the_icon_when_the_mipmaps_exist() {
        // M160: el atributo sin los PNG rompe aapt — solo con icon=true.
        assert!(android_manifest(true, false).contains("android:icon=\"@mipmap/ic_launcher\""));
        assert!(!android_manifest(false, false).contains("android:icon"));
    }

    /// M324 (ray808 #18): `[android] background_audio` declara el servicio de reproducción con sus
    /// permisos, escribe la clase y enciende el flag de MainActivity; sin él, nada de eso existe.
    #[test]
    fn background_audio_adds_the_foreground_service_only_when_asked() {
        let with = android_manifest(false, true);
        assert!(with.contains("android:name=\".RayPlaybackService\""), "{with}");
        assert!(with.contains("android:foregroundServiceType=\"mediaPlayback\""), "{with}");
        for perm in ["FOREGROUND_SERVICE", "FOREGROUND_SERVICE_MEDIA_PLAYBACK", "POST_NOTIFICATIONS"] {
            assert!(with.contains(&format!("android.permission.{perm}")), "{perm}");
        }
        let without = android_manifest(false, false);
        assert!(!without.contains("RayPlaybackService") && !without.contains("FOREGROUND_SERVICE"), "{without}");
        assert!(MAIN_ACTIVITY_JAVA.contains("if (RAY_BACKGROUND_AUDIO) { RayPlaybackService.start(this); }"));
        assert!(RAY_PLAYBACK_SERVICE_JAVA.contains("ServiceInfo.FOREGROUND_SERVICE_TYPE_MEDIA_PLAYBACK"));
        let base = std::env::temp_dir().join(format!("ray_android_audio_{}", std::process::id()));
        for (bg, want) in [(true, true), (false, false)] {
            let _ = std::fs::remove_dir_all(&base);
            write_project(&base, "App", "org.example.app", "1.0.0", "'arm64-v8a'", false, false, bg).unwrap();
            let main = std::fs::read_to_string(base.join("app/src/main/java/org/raylang/shell/MainActivity.java")).unwrap();
            assert_eq!(main.contains("RAY_BACKGROUND_AUDIO = true;"), want, "bg={bg}");
            assert!(!main.contains("/*RAY_BACKGROUND_AUDIO*/"), "sin marcador");
            assert_eq!(base.join("app/src/main/java/org/raylang/shell/RayPlaybackService.java").is_file(), want, "bg={bg}");
        }
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn the_release_signing_is_conditional_and_holds_no_secrets() {
        // M160: el bloque de firma existe pero SOLO aplica con keystore.properties presente;
        // ni una contraseña literal en la plantilla.
        let gradle = app_build_gradle("org.raylang.demo", "1.0.0", "'arm64-v8a'");
        assert!(gradle.contains("rootProject.file('keystore.properties')"), "{gradle}");
        assert!(gradle.contains("signingConfigs"), "{gradle}");
        assert!(gradle.contains("signingConfig signingConfigs.release"), "{gradle}");
        assert!(gradle.contains("if (keystorePropsFile.exists())"), "{gradle}");
        assert!(gradle.contains("keystoreProps['storePassword']"), "{gradle}");
        assert!(!gradle.to_lowercase().contains("password '"), "sin secretos literales");
        // Y el README enseña el flujo completo.
        assert!(README.contains("keytool -genkeypair"), "release flow in README");
        assert!(README.contains("assembleRelease"));
    }
}
