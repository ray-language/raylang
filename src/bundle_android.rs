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
fn android_manifest(icon: bool, background_audio: bool, url_scheme: Option<&str>, exact_alarms: bool, alarm: bool) -> String {
    let icon_attr = if icon { "\n      android:icon=\"@mipmap/ic_launcher\"" } else { "" };
    // M330 D5: el shell de desarrollo abre las URLs de su propio esquema (el QR del enlace);
    // `singleTask` para que un enlace con la app abierta llegue a `onNewIntent` de la actividad
    // viva (con `standard` se apilaría una segunda actividad con otro WebView).
    let launch_mode = if url_scheme.is_some() { "\n        android:launchMode=\"singleTask\"" } else { "" };
    let url_filter = match url_scheme {
        Some(scheme) => format!(
            "\n      <intent-filter>\n        <action android:name=\"android.intent.action.VIEW\" />\n        <category android:name=\"android.intent.category.DEFAULT\" />\n        <category android:name=\"android.intent.category.BROWSABLE\" />\n        <data android:scheme=\"{scheme}\" />\n      </intent-filter>"
        ),
        None => String::new(),
    };
    // M324 (ray808 #18): el foreground service de reproducción y sus permisos (FOREGROUND_SERVICE
    // desde API 28, su tipo mediaPlayback desde API 34, la notificación desde API 33).
    let (audio_perms, audio_service) = if background_audio {
        (
            "\n  <uses-permission android:name=\"android.permission.FOREGROUND_SERVICE\" />\n  <uses-permission android:name=\"android.permission.FOREGROUND_SERVICE_MEDIA_PLAYBACK\" />",
            "\n    <service\n        android:name=\".RayPlaybackService\"\n        android:exported=\"false\"\n        android:foregroundServiceType=\"mediaPlayback\" />",
        )
    } else {
        ("", "")
    };
    // M442 (IDEAS §121): std/notify — POST_NOTIFICATIONS (API 33; se pide en tiempo de ejecución
    // desde el programa, nunca al arrancar) y RECEIVE_BOOT_COMPLETED (normal, sin diálogo) para
    // rearmar las programadas tras reiniciar; el receiver nace deshabilitado y RayNotify lo
    // enciende mientras haya alguna pendiente. `[android] exact_alarms` añade SCHEDULE_EXACT_ALARM.
    let exact_perm = if exact_alarms && !alarm { "\n  <uses-permission android:name=\"android.permission.SCHEDULE_EXACT_ALARM\" />" } else { "" };
    // M448 (IDEAS §122): std/alarm — SOLO si el programa lo importa (Google Play reserva estos
    // permisos a apps de alarma/calendario). `USE_EXACT_ALARM` (API 33+, sin diálogo) y
    // `SCHEDULE_EXACT_ALARM` hasta la 32 (concedido por defecto ahí); el full-screen intent de la
    // pantalla de alarma; el servicio en primer plano que suena; y sus tres componentes.
    let (alarm_perms, alarm_components) = if alarm {
        (
            format!(
                "\n  <uses-permission android:name=\"android.permission.USE_EXACT_ALARM\" />\n  <uses-permission android:name=\"android.permission.SCHEDULE_EXACT_ALARM\"{} />\n  <uses-permission android:name=\"android.permission.USE_FULL_SCREEN_INTENT\" />{}",
                if exact_alarms { "" } else { " android:maxSdkVersion=\"32\"" },
                if background_audio { "" } else { "\n  <uses-permission android:name=\"android.permission.FOREGROUND_SERVICE\" />\n  <uses-permission android:name=\"android.permission.FOREGROUND_SERVICE_MEDIA_PLAYBACK\" />" }
            ),
            "\n    <activity\n        android:name=\".RayAlarmActivity\"\n        android:exported=\"false\"\n        android:showWhenLocked=\"true\"\n        android:turnScreenOn=\"true\"\n        android:excludeFromRecents=\"true\"\n        android:launchMode=\"singleInstance\"\n        android:taskAffinity=\"\"\n        android:theme=\"@style/RayTheme\" />\n    <service\n        android:name=\".RayAlarmService\"\n        android:exported=\"false\"\n        android:foregroundServiceType=\"mediaPlayback\" />\n    <receiver\n        android:name=\".RayAlarmReceiver\"\n        android:exported=\"false\" />",
        )
    } else {
        (String::new(), "")
    };
    format!(
        r#"<?xml version="1.0" encoding="utf-8"?>
<manifest xmlns:android="http://schemas.android.com/apk/res/android">
  <uses-permission android:name="android.permission.INTERNET" />
  <uses-permission android:name="android.permission.POST_NOTIFICATIONS" />
  <uses-permission android:name="android.permission.RECEIVE_BOOT_COMPLETED" />{exact_perm}{alarm_perms}{audio_perms}
  <application
      android:label="@string/app_name"{icon_attr}
      android:networkSecurityConfig="@xml/network_security_config"
      android:theme="@style/RayTheme">
    <activity
        android:name=".MainActivity"
        android:exported="true"{launch_mode}
        android:configChanges="orientation|screenSize|screenLayout|keyboardHidden|uiMode">
      <intent-filter>
        <action android:name="android.intent.action.MAIN" />
        <category android:name="android.intent.category.LAUNCHER" />
      </intent-filter>{url_filter}
    </activity>{audio_service}{alarm_components}
    <receiver
        android:name=".RayNotifyReceiver"
        android:exported="false"
        android:enabled="false">
      <intent-filter>
        <action android:name="android.intent.action.BOOT_COMPLETED" />
        <action android:name="android.intent.action.TIMEZONE_CHANGED" />
        <action android:name="android.intent.action.TIME_SET" />
      </intent-filter>
    </receiver>
  </application>
</manifest>
"#
    )
}

/// rayplay #177: el tema de la Activity sigue al modo claro/oscuro del sistema por el
/// calificador `values-night/`. El WebView deriva `prefers-color-scheme` del atributo
/// `isLightTheme` del tema (API 33+), así que la página lo ve como en iOS y el escritorio.
const STYLES_XML: &str = r#"<?xml version="1.0" encoding="utf-8"?>
<resources>
  <style name="RayTheme" parent="@android:style/Theme.Material.Light.NoActionBar">
    <!-- rayplay #181: iconos oscuros en la barra de estado sobre el fondo claro -->
    <item name="android:windowLightStatusBar">true</item>
  </style>
</resources>
"#;

const STYLES_NIGHT_XML: &str = r#"<?xml version="1.0" encoding="utf-8"?>
<resources>
  <style name="RayTheme" parent="@android:style/Theme.Material.NoActionBar">
    <item name="android:windowLightStatusBar">false</item>
  </style>
</resources>
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
import android.content.Intent;
import android.content.res.Configuration;
import android.graphics.Bitmap;
import android.net.Uri;
import android.os.Bundle;
import android.webkit.JavascriptInterface;
import android.webkit.ValueCallback;
import android.webkit.WebChromeClient;
import android.webkit.WebResourceRequest;
import android.webkit.WebResourceResponse;
import android.webkit.WebView;
import android.webkit.WebViewClient;
import androidx.webkit.WebSettingsCompat;
import androidx.webkit.WebViewCompat;
import androidx.webkit.WebViewFeature;
import java.util.Collections;

public class MainActivity extends Activity {
    // El puente IPC — el MISMO contrato que el user script de WKWebView (window.ray.send /
    // request / _deliver / _deliver_json).
    static final String RAY_SHIM =
        "(function(){var p={},n=0;function e(t){return typeof t==='string'?t:JSON.stringify(t)}"
            + "function q(s){RayAndroid.send(String(s).replace(/\\u0000/g,''))}"
            + "window.ray={send:function(t){q(e(t))},request:function(t){n=n+1;var i=n;"
            + "return new Promise(function(r){p[i]=r;q('\\u0001q\\u0001'+i+'\\u0001'+e(t))})},"
            + "_deliver:function(i,v){var r=p[i];if(r){delete p[i];r(v)}},"
            + "_deliver_json:function(i,t){var r=p[i];if(r){delete p[i];r(JSON.parse(t))}}}})()";

    // El <input type="file"> de la página abre el selector del sistema.
    private static final int RAY_FILE_CHOOSER = 7001;
    private ValueCallback<Uri[]> rayFileCallback = null;
    private WebView rayWeb = null;
    // M382: `ui.pick_file*` del programa → ACTION_OPEN_DOCUMENT; lo elegido se COPIA al cacheDir
    // de la app (un content:// no es una ruta) y las rutas vuelven por RayBridge.pickResult.
    private static final int RAY_PICK = 7002;
    private long rayPickId = 0;
    static MainActivity rayCurrent = null;

    void rayPick(String spec) {
        String[] f = spec.split("\n", -1); // id, kind, title, multiple, extensiones (a,b,c)
        long id = Long.parseLong(f[0]);
        if (rayPickId != 0) {
            RayBridge.pickResult(id, "", "ui: a file picker is already open");
            return;
        }
        rayPickId = id;
        Intent in = new Intent(Intent.ACTION_OPEN_DOCUMENT);
        in.addCategory(Intent.CATEGORY_OPENABLE);
        in.setType("*/*");
        java.util.ArrayList<String> mimes = new java.util.ArrayList<>();
        for (String ext : f[4].split(",")) {
            if (ext.isEmpty()) continue;
            String m = android.webkit.MimeTypeMap.getSingleton().getMimeTypeFromExtension(ext.toLowerCase());
            if (m != null && !mimes.contains(m)) mimes.add(m);
        }
        if (!mimes.isEmpty()) in.putExtra(Intent.EXTRA_MIME_TYPES, mimes.toArray(new String[0]));
        if (f[3].equals("1")) in.putExtra(Intent.EXTRA_ALLOW_MULTIPLE, true);
        if (!f[2].isEmpty()) in.putExtra(Intent.EXTRA_TITLE, f[2]);
        try {
            startActivityForResult(in, RAY_PICK);
        } catch (Exception e) {
            rayPickId = 0;
            RayBridge.pickResult(id, "", "ui: no file picker on this device: " + e);
        }
    }

    private String rayDisplayName(Uri uri) {
        try (android.database.Cursor c = getContentResolver().query(uri, null, null, null, null)) {
            if (c != null && c.moveToFirst()) {
                int i = c.getColumnIndex(android.provider.OpenableColumns.DISPLAY_NAME);
                if (i >= 0 && c.getString(i) != null) return c.getString(i);
            }
        } catch (Exception ignored) {
        }
        String last = uri.getLastPathSegment();
        return last == null ? "file" : last.replace('/', '_');
    }

    private void rayFinishPick(long id, int resultCode, Intent data) {
        java.util.ArrayList<Uri> uris = new java.util.ArrayList<>();
        if (resultCode == RESULT_OK && data != null) {
            if (data.getClipData() != null) {
                for (int i = 0; i < data.getClipData().getItemCount(); i++) uris.add(data.getClipData().getItemAt(i).getUri());
            } else if (data.getData() != null) {
                uris.add(data.getData());
            }
        }
        if (uris.isEmpty()) {
            RayBridge.pickResult(id, "", null); // cancelado
            return;
        }
        // Copiar fuera del hilo principal: un archivo de decenas de MB no debe congelar la UI.
        new Thread(() -> {
            StringBuilder paths = new StringBuilder();
            String err = null;
            java.io.File dir = new java.io.File(getCacheDir(), "ray-picked/" + System.currentTimeMillis());
            dir.mkdirs();
            for (Uri uri : uris) {
                try (java.io.InputStream src = getContentResolver().openInputStream(uri)) {
                    if (src == null) throw new java.io.IOException("no stream for " + uri);
                    java.io.File out = new java.io.File(dir, rayDisplayName(uri));
                    try (java.io.FileOutputStream dst = new java.io.FileOutputStream(out)) {
                        byte[] buf = new byte[1 << 16];
                        int n;
                        while ((n = src.read(buf)) > 0) dst.write(buf, 0, n);
                    }
                    paths.append(out.getAbsolutePath()).append('\n');
                } catch (Exception e) {
                    err = "ui: could not read the picked file: " + e;
                }
            }
            RayBridge.pickResult(id, paths.toString(), err);
        }).start();
    }
    // `[android] background_audio` — foreground service mientras la app está
    // en segundo plano, para que el sistema no mate el proceso (y con él std/audio).
    static final boolean RAY_BACKGROUND_AUDIO = /*RAY_BACKGROUND_AUDIO*/false;

    // rayplay #177: la página sigue al sistema con `prefers-color-scheme`, como en iOS y el
    // escritorio. Desde la API 33 el WebView lo deriva del atributo `isLightTheme` del tema
    // (RayTheme: `values/` claro, `values-night/` oscuro) y basta con no recrear la Activity
    // (`uiMode` en configChanges). En la 29–32 hay que pedirlo con force dark, y SOLO para las
    // páginas que traen tema oscuro propio: sin oscurecer algorítmicamente las demás.
    // rayplay #181: los iconos de las barras del sistema (hora, batería, navegación) siguen al
    // modo — oscuros sobre el fondo claro, claros sobre el oscuro. El tema lo pide al arrancar
    // (`windowLightStatusBar`), pero como la Activity no se recrea al cambiar de modo hay que
    // reaplicarlo a mano: API 30+ por WindowInsetsController; antes, por los flags de systemUi.
    @SuppressWarnings("deprecation")
    private void rayApplyTheme(Configuration cfg) {
        boolean night = (cfg.uiMode & Configuration.UI_MODE_NIGHT_MASK) == Configuration.UI_MODE_NIGHT_YES;
        android.view.View decor = getWindow().getDecorView();
        // El fondo de la ventana (lo que se ve tras las barras del sistema con targetSdk 35) se
        // instanció con el tema del arranque y no sigue al cambio de modo: se vuelve a resolver
        // `colorBackground` del tema vigente y se fija a mano (rayplay #181, segunda vuelta).
        android.util.TypedValue tv = new android.util.TypedValue();
        if (getTheme().resolveAttribute(android.R.attr.colorBackground, tv, true)) {
            getWindow().setBackgroundDrawable(new android.graphics.drawable.ColorDrawable(tv.data));
        }
        if (android.os.Build.VERSION.SDK_INT >= 30) {
            android.view.WindowInsetsController c = decor.getWindowInsetsController();
            if (c != null) {
                int light = android.view.WindowInsetsController.APPEARANCE_LIGHT_STATUS_BARS
                    | android.view.WindowInsetsController.APPEARANCE_LIGHT_NAVIGATION_BARS;
                c.setSystemBarsAppearance(night ? 0 : light, light);
            }
        } else {
            int flags = decor.getSystemUiVisibility();
            int light = android.view.View.SYSTEM_UI_FLAG_LIGHT_STATUS_BAR;
            if (android.os.Build.VERSION.SDK_INT >= 26) { light |= android.view.View.SYSTEM_UI_FLAG_LIGHT_NAVIGATION_BAR; }
            decor.setSystemUiVisibility(night ? (flags & ~light) : (flags | light));
        }
        if (rayWeb != null) { rayApplyDarkMode(rayWeb, night); }
    }

    private static void rayApplyDarkMode(WebView web, boolean night) {
        if (android.os.Build.VERSION.SDK_INT >= 33) { return; }
        if (WebViewFeature.isFeatureSupported(WebViewFeature.FORCE_DARK_STRATEGY)) {
            WebSettingsCompat.setForceDarkStrategy(web.getSettings(), WebSettingsCompat.DARK_STRATEGY_WEB_THEME_DARKENING_ONLY);
        }
        if (WebViewFeature.isFeatureSupported(WebViewFeature.FORCE_DARK)) {
            WebSettingsCompat.setForceDark(web.getSettings(), night ? WebSettingsCompat.FORCE_DARK_ON : WebSettingsCompat.FORCE_DARK_OFF);
        }
    }

    @Override
    public void onConfigurationChanged(Configuration newConfig) {
        super.onConfigurationChanged(newConfig);
        rayApplyTheme(newConfig);
    }

    @Override
    protected void onCreate(Bundle savedInstanceState) {
        super.onCreate(savedInstanceState);
        // El programa corre con cwd=/ y sin HOME — no tendría dónde escribir.
        // HOME = el directorio de datos privado de la app; TMPDIR = su caché (fs.temp_dir()).
        try {
            android.system.Os.setenv("HOME", getFilesDir().getAbsolutePath(), true);
            android.system.Os.setenv("TMPDIR", getCacheDir().getAbsolutePath(), true);
            // El nombre visible de la app, para la pantalla de emparejamiento de la librería de
            // desarrollo (la misma para todos los proyectos).
            android.system.Os.setenv("RAY_DEV_APP_NAME", getApplicationInfo().loadLabel(getPackageManager()).toString(), true);
        } catch (Exception e) { }
        WebView web = new WebView(this);
        web.getSettings().setJavaScriptEnabled(true);
        web.getSettings().setDomStorageEnabled(true);
        web.getSettings().setAllowFileAccess(true);
        rayWeb = web;
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
        // El shim va como script de INICIO DE DOCUMENTO, antes de cualquier
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
                // ray://app/… (y su alias https) se sirve desde el programa.
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
        // El shell apunta a la API 35, donde Android dibuja de borde a borde: sin esto la
        // página queda bajo la barra de estado y el teclado tapa los campos. El WebView va dentro
        // de un FrameLayout que recibe como relleno las barras del sistema, el recorte de la
        // cámara y el teclado (API 30+; antes el sistema ya reserva esas zonas por su cuenta).
        android.widget.FrameLayout frame = new android.widget.FrameLayout(this);
        frame.addView(web, new android.widget.FrameLayout.LayoutParams(
            android.widget.FrameLayout.LayoutParams.MATCH_PARENT,
            android.widget.FrameLayout.LayoutParams.MATCH_PARENT));
        if (android.os.Build.VERSION.SDK_INT >= 30) {
            frame.setOnApplyWindowInsetsListener(new android.view.View.OnApplyWindowInsetsListener() {
                @Override
                public android.view.WindowInsets onApplyWindowInsets(android.view.View v, android.view.WindowInsets insets) {
                    android.graphics.Insets b = insets.getInsets(
                        android.view.WindowInsets.Type.systemBars()
                            | android.view.WindowInsets.Type.displayCutout()
                            | android.view.WindowInsets.Type.ime());
                    v.setPadding(b.left, b.top, b.right, b.bottom);
                    return android.view.WindowInsets.CONSUMED;
                }
            });
        }
        setContentView(frame);
        rayApplyTheme(getResources().getConfiguration()); // tras setContentView: ya hay decor view
        RayBridge.attach(web);
        rayCurrent = this;
        if (RayBridge.lastUrl != null) {
            web.loadUrl(RayBridge.lastUrl); // recreación: el programa sigue vivo, recargar
        }
        RayBridge.startOnce();
        RayAlarm.catchUp(getApplicationContext()); // M448: lo que el reinicio dejó sin armar
        RayAlarm.deliverPending(getApplicationContext()); // M448: alarmas respondidas con la app cerrada
        rayNotificationIntent(getIntent()); // M442: la app arrancó por una notificación pulsada
        /*RAY_DEV_LINK_ONCREATE*/
    }

    // M442 (std/notify): la pulsación de una notificación llega como Intent a esta Activity (viva:
    // onNewIntent; cerrada: onCreate) con el tag del evento `"notification"` en un extra.
    private static void rayNotificationIntent(Intent intent) {
        String tag = intent != null ? intent.getStringExtra(RayNotify.EXTRA_TAP) : null;
        if (tag != null) {
            intent.removeExtra(RayNotify.EXTRA_TAP);
            RayBridge.pushEvent("notification", 0, tag);
        }
    }

    @Override
    protected void onNewIntent(Intent intent) {
        super.onNewIntent(intent);
        setIntent(intent);
        rayNotificationIntent(intent);
        /*RAY_DEV_LINK_NEWINTENT*/
    }

    // M442: el diálogo de POST_NOTIFICATIONS (Android 13+) lo abre std/notify.request_permission;
    // mientras está delante la Activity se pausa y NO debe salir un `lifecycle background`
    // fantasma (findings #99).
    boolean raySuppressLifecycle = false;

    @Override
    public void onRequestPermissionsResult(int requestCode, String[] permissions, int[] grantResults) {
        super.onRequestPermissionsResult(requestCode, permissions, grantResults);
        if (requestCode == RayNotify.PERMISSION_REQUEST) { RayNotify.permissionResult(this, grantResults); }
    }

    // Revisión 1.27.91: si la Activity muere con el diálogo de permiso abierto, la petición del
    // programa recibe su Err en vez de esperar para siempre (y la siguiente no ve «already open»).
    @Override
    public void onDestroy() {
        RayNotify.activityGone();
        super.onDestroy();
    }
    /*RAY_DEV_LINK_METHOD*/

    @Override
    protected void onActivityResult(int requestCode, int resultCode, Intent data) {
        if (requestCode == RAY_FILE_CHOOSER) {
            if (rayFileCallback != null) {
                rayFileCallback.onReceiveValue(WebChromeClient.FileChooserParams.parseResult(resultCode, data));
                rayFileCallback = null;
            }
            return;
        }
        if (requestCode == RAY_PICK) {
            long id = rayPickId;
            rayPickId = 0;
            if (id != 0) rayFinishPick(id, resultCode, data);
            return;
        }
        super.onActivityResult(requestCode, resultCode, data);
    }

    @Override
    protected void onPause() {
        super.onPause();
        if (raySuppressLifecycle) { return; } // el diálogo de permisos, no el usuario
        if (RAY_BACKGROUND_AUDIO) { RayPlaybackService.start(this); }
        RayBridge.pushEvent("lifecycle", 0, "background");
    }

    @Override
    protected void onResume() {
        super.onResume();
        if (raySuppressLifecycle) { raySuppressLifecycle = false; return; }
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
/// M325 (findings #99): la clase se escribe SIEMPRE (MainActivity la referencia tras la constante
/// `RAY_BACKGROUND_AUDIO`; sin ella el proyecto no compilaba) y el permiso POST_NOTIFICATIONS no
/// se pide al arrancar: el diálogo pausaba la actividad y el programa veía un `lifecycle`
/// `background` fantasma. Sin el permiso (Android 13+) el servicio corre igual, solo que sin
/// notificación visible; el usuario puede concederlo en Ajustes.
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

/// M404 (rayplay #170): `std/media` en Android — un `MediaSession` del framework (API 21, sin
/// AndroidX) con metadatos, estado y acciones, y una notificación `MediaStyle` ligada a la sesión:
/// es lo que pinta los controles en la pantalla de bloqueo y en los ajustes rápidos (desde
/// Android 13 solo aparecen por una notificación MediaStyle; las notificaciones de una sesión
/// activa están exentas del permiso POST_NOTIFICATIONS). Las órdenes vuelven como eventos
/// `("media", 0, tag)` por `RayBridge.pushEvent`; `KEYCODE_MEDIA_PLAY_PAUSE` llega como
/// onPlay/onPause según el estado, así que un programa que solo declara "toggle" lo recibe así.
/// Todo en el main thread (lo postea `RayBridge.onMedia`).
const RAY_MEDIA_JAVA: &str = r#"package org.raylang.shell;

import android.app.Notification;
import android.app.NotificationChannel;
import android.app.NotificationManager;
import android.app.PendingIntent;
import android.content.Context;
import android.content.Intent;
import android.graphics.Bitmap;
import android.graphics.BitmapFactory;
import android.media.MediaMetadata;
import android.media.session.MediaSession;
import android.media.session.PlaybackState;
import android.os.Build;
import android.util.Base64;

final class RayMedia {
    static final String CHANNEL = "ray_media";
    static final int NOTIFICATION_ID = 2;

    private static MediaSession session;
    private static long actions = 0;
    private static boolean wantsPlay, wantsPause, wantsToggle;
    private static String title = "", artist = "", album = "";
    private static long durationMs = 0, positionMs = 0;
    private static boolean playing = false;
    private static Bitmap art;

    static void apply(String op, String arg) {
        MainActivity a = MainActivity.rayCurrent;
        if (a == null) {
            return;
        }
        Context c = a.getApplicationContext();
        switch (op) {
            case "media_controls": controls(c, arg); break;
            case "media_now_playing": nowPlaying(c, arg); break;
            case "media_clear": clear(c); break;
            default: break;
        }
    }

    private static MediaSession session(Context c) {
        if (session == null) {
            session = new MediaSession(c, "ray");
            session.setCallback(new MediaSession.Callback() {
                @Override public void onPlay() { RayBridge.pushEvent("media", 0, wantsPlay ? "play" : "toggle"); }
                @Override public void onPause() { RayBridge.pushEvent("media", 0, wantsPause ? "pause" : "toggle"); }
                @Override public void onSkipToNext() { RayBridge.pushEvent("media", 0, "next"); }
                @Override public void onSkipToPrevious() { RayBridge.pushEvent("media", 0, "previous"); }
                @Override public void onStop() { RayBridge.pushEvent("media", 0, "stop"); }
                @Override public void onSeekTo(long pos) { RayBridge.pushEvent("media", 0, "seek:" + pos); }
            });
            session.setActive(true);
        }
        return session;
    }

    private static void controls(Context c, String list) {
        long a = 0;
        wantsPlay = wantsPause = wantsToggle = false;
        for (String cmd : list.split(",")) {
            switch (cmd.trim()) {
                case "play": a |= PlaybackState.ACTION_PLAY; wantsPlay = true; break;
                case "pause": a |= PlaybackState.ACTION_PAUSE; wantsPause = true; break;
                case "toggle": a |= PlaybackState.ACTION_PLAY_PAUSE; wantsToggle = true; break;
                case "next": a |= PlaybackState.ACTION_SKIP_TO_NEXT; break;
                case "previous": a |= PlaybackState.ACTION_SKIP_TO_PREVIOUS; break;
                case "stop": a |= PlaybackState.ACTION_STOP; break;
                case "seek": a |= PlaybackState.ACTION_SEEK_TO; break;
                default: break;
            }
        }
        if (wantsToggle) { a |= PlaybackState.ACTION_PLAY | PlaybackState.ACTION_PAUSE; }
        actions = a;
        updateState(c);
    }

    private static void nowPlaying(Context c, String arg) {
        String[] f = arg.split("\n", -1);
        title = f.length > 0 ? f[0] : "";
        artist = f.length > 1 ? f[1] : "";
        album = f.length > 2 ? f[2] : "";
        durationMs = f.length > 3 ? parseLong(f[3]) : 0;
        positionMs = f.length > 4 ? parseLong(f[4]) : 0;
        playing = f.length > 5 && "1".equals(f[5].trim());
        art = null;
        if (f.length > 6 && !f[6].isEmpty()) {
            try {
                byte[] png = Base64.decode(f[6], Base64.DEFAULT);
                art = BitmapFactory.decodeByteArray(png, 0, png.length);
            } catch (IllegalArgumentException e) {
                art = null;
            }
        }
        MediaMetadata.Builder m = new MediaMetadata.Builder()
            .putString(MediaMetadata.METADATA_KEY_TITLE, title)
            .putString(MediaMetadata.METADATA_KEY_ARTIST, artist)
            .putString(MediaMetadata.METADATA_KEY_ALBUM, album);
        if (durationMs > 0) { m.putLong(MediaMetadata.METADATA_KEY_DURATION, durationMs); }
        if (art != null) { m.putBitmap(MediaMetadata.METADATA_KEY_ALBUM_ART, art); }
        session(c).setMetadata(m.build());
        updateState(c);
        notifyNowPlaying(c);
    }

    private static void updateState(Context c) {
        if (session == null && title.isEmpty()) {
            return; // controls antes de now_playing: la sesión nace con los metadatos
        }
        PlaybackState st = new PlaybackState.Builder()
            .setActions(actions)
            .setState(playing ? PlaybackState.STATE_PLAYING : PlaybackState.STATE_PAUSED, positionMs, playing ? 1f : 0f)
            .build();
        session(c).setPlaybackState(st);
    }

    private static void notifyNowPlaying(Context c) {
        NotificationManager nm = (NotificationManager) c.getSystemService(Context.NOTIFICATION_SERVICE);
        if (nm == null) {
            return;
        }
        Notification.Builder b;
        if (Build.VERSION.SDK_INT >= 26) {
            nm.createNotificationChannel(new NotificationChannel(CHANNEL, "Now Playing", NotificationManager.IMPORTANCE_LOW));
            b = new Notification.Builder(c, CHANNEL);
        } else {
            b = new Notification.Builder(c);
        }
        Intent open = new Intent(c, MainActivity.class);
        open.setFlags(Intent.FLAG_ACTIVITY_SINGLE_TOP);
        b.setContentTitle(title)
            .setContentText(artist.isEmpty() ? album : (album.isEmpty() ? artist : artist + " — " + album))
            .setSmallIcon(android.R.drawable.ic_media_play)
            .setContentIntent(PendingIntent.getActivity(c, 0, open, PendingIntent.FLAG_IMMUTABLE))
            .setOngoing(playing)
            .setVisibility(Notification.VISIBILITY_PUBLIC)
            .setStyle(new Notification.MediaStyle().setMediaSession(session(c).getSessionToken()));
        if (art != null) { b.setLargeIcon(art); }
        nm.notify(NOTIFICATION_ID, b.build());
    }

    private static void clear(Context c) {
        NotificationManager nm = (NotificationManager) c.getSystemService(Context.NOTIFICATION_SERVICE);
        if (nm != null) { nm.cancel(NOTIFICATION_ID); }
        if (session != null) {
            session.setActive(false);
            session.release();
            session = null;
        }
        title = ""; art = null; playing = false;
    }

    private static long parseLong(String s) {
        try { return Long.parseLong(s.trim()); } catch (NumberFormatException e) { return 0; }
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

    // M448: ¿corre ya el programa raylang? (RayAlarm guarda la respuesta de una alarma si no).
    static boolean isStarted() {
        return started;
    }

    static void startOnce() {
        if (!started) {
            started = true;
            capabilities(1); // Este shell sirve ray://app (RayScheme)
            start(); // registra los handlers y lanza el programa raylang en su hilo
        }
    }

    public static native void capabilities(int caps);

    public static native int start();

    public static native void pushEvent(String kind, long window, String tag);

    // M382: la respuesta del selector de archivos (ver MainActivity.rayPick).
    public static native void pickResult(long id, String paths, String err);

    // M442: la respuesta a una petición de std/notify (ver onRequest).
    public static native void requestResult(long id, String value, String err);
    /*RAY_DEV_NATIVE*/

    // El esquema ray://app/… (ver RayScheme).
    public static native long schemeOpen(String url, String method, String range, String ifNoneMatch);

    public static native int schemeStatus(long handle);

    public static native String schemeHeaders(long handle);

    public static native byte[] schemeRead(long handle);

    public static native void schemeClose(long handle);

    // Llamados desde NATIVO (el hilo del programa): siempre postear al main thread.
    public static void onOpen(String title, String url) {
        String target = RayScheme.alias(url); // ray://app/… carga por su alias https
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

    // rayreq #263: `ui.open_url` — la URL en el navegador del sistema (Intent ACTION_VIEW), en el
    // main thread. Sin una app que la abra, se anota en logcat (el programa ya recibió Ok).
    public static void onOpenUrl(String url) {
        MAIN.post(() -> {
            android.content.Context c = webView != null ? webView.getContext() : MainActivity.rayCurrent;
            if (c == null) {
                return;
            }
            android.content.Intent i = new android.content.Intent(android.content.Intent.ACTION_VIEW, android.net.Uri.parse(url));
            if (!(c instanceof android.app.Activity)) {
                i.addFlags(android.content.Intent.FLAG_ACTIVITY_NEW_TASK);
            }
            try {
                c.startActivity(i);
            } catch (android.content.ActivityNotFoundException e) {
                android.util.Log.w("ray", "open_url: no app opens " + url);
            }
        });
    }

    // M445 (pepa #331): `ui.haptic` — una vibración corta por HapticFeedbackConstants sobre la
    // vista de la página (sigue el ajuste «respuesta táctil» del usuario), en el main thread.
    public static void onHaptic(String kind) {
        MAIN.post(() -> {
            android.view.View v = webView;
            if (v == null) {
                return;
            }
            int api = android.os.Build.VERSION.SDK_INT;
            int c;
            switch (kind) {
                case "light": case "soft": case "selection":
                    c = android.view.HapticFeedbackConstants.CLOCK_TICK; break;
                case "heavy": case "rigid":
                    c = android.view.HapticFeedbackConstants.LONG_PRESS; break;
                case "success":
                    c = api >= 30 ? android.view.HapticFeedbackConstants.CONFIRM : android.view.HapticFeedbackConstants.CONTEXT_CLICK; break;
                case "warning": case "error":
                    c = api >= 30 ? android.view.HapticFeedbackConstants.REJECT : android.view.HapticFeedbackConstants.LONG_PRESS; break;
                default:
                    c = android.view.HapticFeedbackConstants.CONTEXT_CLICK; break;
            }
            v.performHapticFeedback(c);
        });
    }

    // M404: `std/media` — Now Playing y controles del sistema (MediaSession), en el main thread.
    public static void onMedia(String op, String arg) {
        MAIN.post(() -> RayMedia.apply(op, arg));
    }

    // M442: una petición de `std/notify` (permiso, mostrar, programar…): RayNotify la atiende en el
    // main thread y contesta con requestResult. El id viaja como texto (el relay solo cruza Strings).
    public static void onRequest(String id, String op, String arg) {
        MAIN.post(() -> {
            android.content.Context c = webView != null ? webView.getContext().getApplicationContext()
                : (MainActivity.rayCurrent != null ? MainActivity.rayCurrent.getApplicationContext() : null);
            if (c == null) {
                requestResult(Long.parseLong(id), null, "notify: the shell has no context yet");
                return;
            }
            if (op.startsWith("alarm_")) { // M448: std/alarm
                RayAlarm.apply(c, Long.parseLong(id), op, arg);
            } else {
                RayNotify.apply(c, Long.parseLong(id), op, arg);
            }
        });
    }

    // M382: `ui.pick_file*` — el programa pide el selector; la Activity lo presenta.
    public static void onPick(String spec) {
        MAIN.post(() -> {
            MainActivity a = MainActivity.rayCurrent;
            if (a == null) {
                pickResult(Long.parseLong(spec.split("\n", -1)[0]), "", "ui: no activity to present the file picker");
            } else {
                a.rayPick(spec);
            }
        });
    }
}
"#;

/// M442 (IDEAS §121): `std/notify` en Android. TODO el trabajo vive aquí, en Java, para que las
/// notificaciones programadas se muestren también con el proceso raylang muerto: `AlarmManager`
/// despierta a `RayNotifyReceiver`, que lee la notificación guardada en `SharedPreferences`
/// (`ray_notify`: id → `disparo|intervalo|campos`) y la pinta; tras un reinicio el receiver
/// (BOOT_COMPLETED, habilitado solo mientras haya pendientes) las rearma. El permiso
/// POST_NOTIFICATIONS (API 33) se pide SOLO cuando el programa llama `request_permission` — nunca al
/// arrancar (findings #99) — y MainActivity silencia el `lifecycle` fantasma del diálogo. Las
/// alarmas son inexactas salvo `[android] exact_alarms = true` (SCHEDULE_EXACT_ALARM, si el usuario
/// lo permite). Una pulsación (la notificación o una de sus acciones) abre MainActivity con el tag
/// del evento en un extra; Android no tiene API de badge (`notify_badge` es `Ok` sin efecto).
const RAY_NOTIFY_JAVA: &str = r#"package org.raylang.shell;

import android.app.AlarmManager;
import android.app.Notification;
import android.app.NotificationChannel;
import android.app.NotificationManager;
import android.app.PendingIntent;
import android.content.ComponentName;
import android.content.Context;
import android.content.Intent;
import android.content.SharedPreferences;
import android.content.pm.PackageManager;
import android.os.Build;
import java.util.ArrayList;
import java.util.Calendar;
import java.util.Map;

public final class RayNotify {
    static final boolean EXACT_ALARMS = /*RAY_EXACT_ALARMS*/false;
    static final String PREFS = "ray_notify";
    static final String STATE = "ray_notify_state";
    static final String EXTRA_TAP = "org.raylang.shell.NOTIFICATION";
    static final String ACTION_FIRE = "org.raylang.shell.NOTIFY_FIRE";
    static final String POST = "android.permission.POST_NOTIFICATIONS";
    static final String SEP = "\u001f";
    static final String ACT = "\u001e";
    static final String FIELD = "\u001d";
    static final int PERMISSION_REQUEST = 7003;
    static long pendingPermission = 0;

    // Una petición de std/notify (RayBridge.onRequest), en el main thread. Los campos de una
    // notificación: trigger, id, title, body, subtitle, channel, group, data, sound, badge,
    // interruption, actions (ver ray_runtime::notify).
    static void apply(Context c, long id, String op, String arg) {
        try {
            switch (op) {
                case "notify_permission":
                    permission(c, id);
                    return;
                case "notify_status":
                    RayBridge.requestResult(id, status(c), null);
                    return;
                case "notify_channel": {
                    String[] f = arg.split(SEP, -1);
                    channel(c, f[0], f[1], f[2], f[3], f.length > 4 ? f[4] : "");
                    RayBridge.requestResult(id, "", null);
                    return;
                }
                case "notify_show": {
                    String[] f = arg.split(SEP, -1);
                    if (f[0].isEmpty() || f[0].equals("now")) { show(c, f); } else { schedule(c, f); }
                    RayBridge.requestResult(id, f[1], null);
                    return;
                }
                case "notify_cancel":
                    cancel(c, arg);
                    RayBridge.requestResult(id, "", null);
                    return;
                case "notify_cancel_all":
                    for (String k : new ArrayList<>(prefs(c).getAll().keySet())) { cancel(c, k); }
                    RayBridge.requestResult(id, "", null);
                    return;
                case "notify_pending":
                    RayBridge.requestResult(id, String.join("\n", prefs(c).getAll().keySet()), null);
                    return;
                case "notify_clear":
                    manager(c).cancelAll();
                    RayBridge.requestResult(id, "", null);
                    return;
                case "notify_badge":
                    RayBridge.requestResult(id, "", null); // sin API de badge en Android
                    return;
                default:
                    RayBridge.requestResult(id, null, "notify: unknown op '" + op + "'");
            }
        } catch (Exception e) {
            RayBridge.requestResult(id, null, "notify: " + e);
        }
    }

    static NotificationManager manager(Context c) {
        return (NotificationManager) c.getSystemService(Context.NOTIFICATION_SERVICE);
    }

    static SharedPreferences prefs(Context c) {
        return c.getSharedPreferences(PREFS, Context.MODE_PRIVATE);
    }

    static boolean granted(Context c) {
        return Build.VERSION.SDK_INT < 33 || c.checkSelfPermission(POST) == PackageManager.PERMISSION_GRANTED;
    }

    static String status(Context c) {
        if (!granted(c)) {
            return c.getSharedPreferences(STATE, Context.MODE_PRIVATE).getBoolean("asked", false) ? "denied" : "not_determined";
        }
        return manager(c).areNotificationsEnabled() ? "granted" : "denied";
    }

    static void permission(Context c, long id) {
        if (granted(c)) {
            RayBridge.requestResult(id, status(c), null);
            return;
        }
        MainActivity a = MainActivity.rayCurrent;
        if (a == null) {
            RayBridge.requestResult(id, "denied", null);
            return;
        }
        if (pendingPermission != 0) {
            RayBridge.requestResult(id, null, "notify: a permission request is already open");
            return;
        }
        pendingPermission = id;
        c.getSharedPreferences(STATE, Context.MODE_PRIVATE).edit().putBoolean("asked", true).apply();
        a.raySuppressLifecycle = true;
        a.requestPermissions(new String[] { POST }, PERMISSION_REQUEST);
    }

    static void activityGone() {
        long id = pendingPermission;
        pendingPermission = 0;
        if (id != 0) { RayBridge.requestResult(id, null, "notify: the activity was destroyed before the user answered"); }
    }

    static void permissionResult(Context c, int[] grantResults) {
        long id = pendingPermission;
        pendingPermission = 0;
        if (id == 0) { return; }
        boolean ok = grantResults.length > 0 && grantResults[0] == PackageManager.PERMISSION_GRANTED;
        RayBridge.requestResult(id, ok ? "granted" : "denied", null);
    }

    // M447 (pepa #331): un sonido de `[android] sounds` (res/raw/<nombre>) como URI de recurso.
    static android.net.Uri soundUri(Context c, String sound) {
        if (sound.isEmpty()) { return null; }
        int dot = sound.lastIndexOf('.');
        if (dot > 0) { sound = sound.substring(0, dot); } // "alarm.caf" (forma de iOS) → recurso "alarm"
        int res = c.getResources().getIdentifier(sound.toLowerCase(), "raw", c.getPackageName());
        if (res == 0) {
            android.util.Log.w("ray", "notify: sound '" + sound + "' is not in res/raw (list the file in [android] sounds of ray.toml)");
            return null;
        }
        return android.net.Uri.parse("android.resource://" + c.getPackageName() + "/" + res);
    }

    static void channel(Context c, String id, String name, String description, String importance, String sound) {
        if (Build.VERSION.SDK_INT < 26) { return; }
        int imp = importance.equals("min") ? NotificationManager.IMPORTANCE_MIN
            : importance.equals("low") ? NotificationManager.IMPORTANCE_LOW
            : importance.equals("high") ? NotificationManager.IMPORTANCE_HIGH
            : NotificationManager.IMPORTANCE_DEFAULT;
        NotificationChannel ch = new NotificationChannel(id, name, imp);
        if (!description.isEmpty()) { ch.setDescription(description); }
        android.net.Uri uri = soundUri(c, sound);
        if (uri != null) {
            // El sonido del canal es fijo una vez creado (como la importancia): el usuario lo cambia en Ajustes.
            ch.setSound(uri, new android.media.AudioAttributes.Builder()
                .setUsage(android.media.AudioAttributes.USAGE_NOTIFICATION)
                .setContentType(android.media.AudioAttributes.CONTENT_TYPE_SONIFICATION).build());
        }
        manager(c).createNotificationChannel(ch); // existente: actualiza nombre y descripción
    }

    static void ensureChannel(Context c, String id) {
        if (Build.VERSION.SDK_INT >= 26 && manager(c).getNotificationChannel(id) == null) {
            channel(c, id, id.equals("default") ? "Notifications" : id, "", "default", "");
        }
    }

    static int smallIcon(Context c) {
        int icon = c.getApplicationInfo().icon;
        return icon != 0 ? icon : android.R.drawable.ic_dialog_info;
    }

    @SuppressWarnings("deprecation")
    static void show(Context c, String[] f) {
        String id = f[1];
        String channel = f[5].isEmpty() ? "default" : f[5];
        ensureChannel(c, channel);
        Notification.Builder b = Build.VERSION.SDK_INT >= 26 ? new Notification.Builder(c, channel) : new Notification.Builder(c);
        b.setContentTitle(f[2]).setContentText(f[3]).setSmallIcon(smallIcon(c)).setAutoCancel(true);
        if (!f[3].isEmpty()) { b.setStyle(new Notification.BigTextStyle().bigText(f[3])); }
        if (!f[4].isEmpty()) { b.setSubText(f[4]); }
        if (!f[6].isEmpty()) { b.setGroup(f[6]); }
        int badge = -1;
        try { badge = Integer.parseInt(f[9].trim()); } catch (NumberFormatException ignored) { }
        if (badge >= 0) { b.setNumber(badge); }
        if (Build.VERSION.SDK_INT < 26) {
            android.net.Uri custom = f.length > 12 ? soundUri(c, f[12]) : null;
            if (custom != null) { b.setSound(custom); } else if (f[8].equals("1")) { b.setDefaults(Notification.DEFAULT_SOUND); }
            b.setPriority(f[10].equals("passive") ? Notification.PRIORITY_LOW : f[10].equals("active") ? Notification.PRIORITY_DEFAULT : Notification.PRIORITY_HIGH);
        }
        b.setContentIntent(tapIntent(c, id, "", f[7]));
        if (f.length > 11 && !f[11].isEmpty()) {
            for (String a : f[11].split(ACT, -1)) {
                if (a.isEmpty()) { continue; }
                String[] p = a.split(FIELD, -1);
                String title = p.length > 1 ? p[1] : p[0];
                android.graphics.drawable.Icon icon = android.graphics.drawable.Icon.createWithResource(c, android.R.drawable.ic_menu_info_details);
                b.addAction(new Notification.Action.Builder(icon, title, tapIntent(c, id, p[0], f[7])).build());
            }
        }
        manager(c).notify(id, 0, b.build());
    }

    // El tag del evento "notification": el id a secas, o `id\taction\ttext\tdata` (ver notify::tap_tag).
    static PendingIntent tapIntent(Context c, String id, String action, String data) {
        Intent i = new Intent(c, MainActivity.class);
        i.setFlags(Intent.FLAG_ACTIVITY_SINGLE_TOP | Intent.FLAG_ACTIVITY_CLEAR_TOP);
        i.putExtra(EXTRA_TAP, (action.isEmpty() && data.isEmpty()) ? id : id + "\t" + action + "\t\t" + data);
        int code = (id + "\u0000" + action).hashCode();
        return PendingIntent.getActivity(c, code, i, PendingIntent.FLAG_UPDATE_CURRENT | PendingIntent.FLAG_IMMUTABLE);
    }

    // {instante del disparo (ms), intervalo (ms; 0 = una vez)} para `after:s:r`, `at:unix`, `daily:h:m`.
    static long[] nextTrigger(String trigger, long now) {
        String[] p = trigger.split(":", -1);
        switch (p[0]) {
            case "after": {
                long secs = Long.parseLong(p[1]);
                boolean repeat = p.length > 2 && p[2].equals("1");
                return new long[] { now + secs * 1000, repeat ? secs * 1000 : 0 };
            }
            case "at":
                return new long[] { Long.parseLong(p[1]) * 1000, 0 };
            case "daily": {
                Calendar cal = Calendar.getInstance();
                cal.setTimeInMillis(now);
                cal.set(Calendar.HOUR_OF_DAY, Integer.parseInt(p[1]));
                cal.set(Calendar.MINUTE, Integer.parseInt(p[2]));
                cal.set(Calendar.SECOND, 0);
                cal.set(Calendar.MILLISECOND, 0);
                if (cal.getTimeInMillis() <= now) { cal.add(Calendar.DAY_OF_YEAR, 1); }
                return new long[] { cal.getTimeInMillis(), 24L * 3600 * 1000 };
            }
            default:
                throw new IllegalArgumentException("unknown trigger '" + trigger + "'");
        }
    }

    static void schedule(Context c, String[] f) {
        String id = f[1];
        long[] t = nextTrigger(f[0], System.currentTimeMillis());
        prefs(c).edit().putString(id, t[0] + "|" + t[1] + "|" + String.join(SEP, f)).apply();
        arm(c, id, t[0]);
    }

    static PendingIntent firePending(Context c, String id) {
        Intent i = new Intent(c, RayNotifyReceiver.class);
        i.setAction(ACTION_FIRE);
        i.setData(android.net.Uri.parse("raynotify://" + android.net.Uri.encode(id))); // un PendingIntent por id
        i.putExtra("id", id);
        return PendingIntent.getBroadcast(c, id.hashCode(), i, PendingIntent.FLAG_UPDATE_CURRENT | PendingIntent.FLAG_IMMUTABLE);
    }

    static void arm(Context c, String id, long at) {
        bootReceiver(c, true);
        AlarmManager am = (AlarmManager) c.getSystemService(Context.ALARM_SERVICE);
        PendingIntent pi = firePending(c, id);
        boolean exact = EXACT_ALARMS && (Build.VERSION.SDK_INT < 31 || am.canScheduleExactAlarms());
        if (exact) { am.setExactAndAllowWhileIdle(AlarmManager.RTC_WAKEUP, at, pi); } else { am.setAndAllowWhileIdle(AlarmManager.RTC_WAKEUP, at, pi); }
    }

    static void cancel(Context c, String id) {
        AlarmManager am = (AlarmManager) c.getSystemService(Context.ALARM_SERVICE);
        am.cancel(firePending(c, id));
        prefs(c).edit().remove(id).apply();
        manager(c).cancel(id, 0);
        syncBootReceiver(c);
    }

    // Un disparo (desde el receiver): mostrar y, si repite, rearmar la siguiente.
    static void fire(Context c, String id) {
        String rec = prefs(c).getString(id, null);
        if (rec == null) { return; }
        String[] parts = rec.split("\\|", 3);
        long at = Long.parseLong(parts[0]);
        long interval = Long.parseLong(parts[1]);
        show(c, parts[2].split(SEP, -1));
        if (interval > 0) {
            long now = System.currentTimeMillis();
            long next = at + interval;
            while (next <= now) { next += interval; }
            prefs(c).edit().putString(id, next + "|" + interval + "|" + parts[2]).apply();
            arm(c, id, next);
        } else {
            prefs(c).edit().remove(id).apply();
            syncBootReceiver(c);
        }
    }

    // Tras reiniciar: lo vencido se dispara ya (y repite si toca), lo demás se rearma.
    static void rearm(Context c) {
        long now = System.currentTimeMillis();
        for (Map.Entry<String, ?> e : new java.util.HashMap<>(prefs(c).getAll()).entrySet()) {
            String[] parts = String.valueOf(e.getValue()).split("\\|", 3);
            long at = Long.parseLong(parts[0]);
            if (at <= now) { fire(c, e.getKey()); } else { arm(c, e.getKey(), at); }
        }
    }

    // M448: el receiver de arranque sigue encendido mientras quede algo que rearmar — una
    // notificación programada o una alarma de std/alarm.
    static void syncBootReceiver(Context c) {
        bootReceiver(c, !prefs(c).getAll().isEmpty() || !RayAlarm.prefs(c).getAll().isEmpty());
    }

    static void bootReceiver(Context c, boolean on) {
        c.getPackageManager().setComponentEnabledSetting(
            new ComponentName(c, RayNotifyReceiver.class),
            on ? PackageManager.COMPONENT_ENABLED_STATE_ENABLED : PackageManager.COMPONENT_ENABLED_STATE_DISABLED,
            PackageManager.DONT_KILL_APP);
    }
}
"#;

/// M448 (IDEAS §122, pepa #341): `std/alarm` en Android. `RayAlarm` programa con
/// `AlarmManager.setAlarmClock` (exacta, visible en la barra de estado, exenta de Doze), guarda
/// cada alarma en `SharedPreferences` (se rearma tras reiniciar por `RayNotifyReceiver`) y, al
/// vencer, `RayAlarmReceiver` arranca `RayAlarmService`: un servicio en primer plano que
/// reproduce el sonido en bucle con el volumen de ALARMA y publica la notificación con el
/// full-screen intent a `RayAlarmActivity`. La respuesta («stop», «snooze», «timeout») va al
/// programa como evento `"alarm"`; si el programa no está corriendo (app cerrada), se guarda y
/// se entrega en el siguiente arranque.
const RAY_ALARM_JAVA: &str = r#"package org.raylang.shell;

import android.app.AlarmManager;
import android.app.PendingIntent;
import android.content.Context;
import android.content.Intent;
import android.content.SharedPreferences;
import android.os.Build;
import java.util.ArrayList;
import java.util.Calendar;
import java.util.Map;
import java.util.TreeMap;

public final class RayAlarm {
    static final boolean ENABLED = /*RAY_ALARM*/false;
    static final String PREFS = "ray_alarm";
    static final String RESPONSES = "ray_alarm_responses";
    static final String ACTION_FIRE = "org.raylang.shell.ALARM_FIRE";
    static final String ACTION_STOP = "org.raylang.shell.ALARM_STOP";
    static final String ACTION_SNOOZE = "org.raylang.shell.ALARM_SNOOZE";
    static final String ACTION_DONE = "org.raylang.shell.ALARM_DONE";
    static final String SEP = "\u001f";
    static final String SNOOZE = "|snooze"; // un id no puede llevar `|`: la clave de la posposición no choca

    // Una petición de std/alarm (RayBridge.onRequest), en el main thread. Los campos de una
    // alarma: trigger, id, title, sound, snooze_minutes, stop_label, data, max_minutes (ver
    // ray_runtime::alarm).
    static void apply(Context c, long id, String op, String arg) {
        try {
            if (!ENABLED) {
                if (op.equals("alarm_status")) {
                    RayBridge.requestResult(id, "unsupported", null);
                } else {
                    RayBridge.requestResult(id, null, "alarm: this shell was bundled without std/alarm (the program did not import it); regenerate it with `ray bundle --android`");
                }
                return;
            }
            switch (op) {
                case "alarm_permission":
                    if (RayNotify.granted(c)) { RayBridge.requestResult(id, status(c), null); } else { RayNotify.permission(c, id); }
                    return;
                case "alarm_status":
                    RayBridge.requestResult(id, status(c), null);
                    return;
                case "alarm_schedule": {
                    String[] f = arg.split(SEP, -1);
                    schedule(c, f);
                    RayBridge.requestResult(id, f[1], null);
                    return;
                }
                case "alarm_cancel":
                    cancel(c, arg.trim());
                    RayBridge.requestResult(id, "", null);
                    return;
                case "alarm_cancel_all":
                    for (String k : pending(c)) { cancel(c, k); }
                    RayAlarmService.stopAny(c); // también la que esté sonando ahora
                    RayBridge.requestResult(id, "", null);
                    return;
                case "alarm_pending":
                    RayBridge.requestResult(id, String.join("\n", pending(c)), null);
                    return;
                default:
                    RayBridge.requestResult(id, null, "alarm: unknown op '" + op + "'");
            }
        } catch (Exception e) {
            RayBridge.requestResult(id, null, "alarm: " + e.getMessage());
        }
    }

    static SharedPreferences prefs(Context c) {
        return c.getSharedPreferences(PREFS, Context.MODE_PRIVATE);
    }

    static ArrayList<String> pending(Context c) {
        // Una alarma pospuesta sigue pendiente aunque solo quede su clave `id|snooze`.
        java.util.TreeSet<String> ids = new java.util.TreeSet<>();
        for (String k : prefs(c).getAll().keySet()) {
            ids.add(k.endsWith(SNOOZE) ? k.substring(0, k.length() - SNOOZE.length()) : k);
        }
        return new ArrayList<>(ids);
    }

    static boolean canExact(Context c) {
        AlarmManager am = (AlarmManager) c.getSystemService(Context.ALARM_SERVICE);
        return Build.VERSION.SDK_INT < 31 || am.canScheduleExactAlarms();
    }

    static String status(Context c) {
        return canExact(c) ? RayNotify.status(c) : "denied";
    }

    // El siguiente instante (ms) de `at:unix`, `daily:h:m` o `weekly:d,d:h:m` (días ISO: 1 = lunes).
    static long next(String trigger, long now) {
        String[] p = trigger.split(":", -1);
        if (p[0].equals("at")) { return Long.parseLong(p[1]) * 1000; }
        boolean weekly = p[0].equals("weekly");
        int hour = Integer.parseInt(p[weekly ? 2 : 1]);
        int minute = Integer.parseInt(p[weekly ? 3 : 2]);
        String days = weekly ? "," + p[1] + "," : null;
        for (int offset = 0; offset <= 7; offset++) {
            Calendar cal = Calendar.getInstance();
            cal.setTimeInMillis(now);
            cal.add(Calendar.DAY_OF_YEAR, offset);
            cal.set(Calendar.HOUR_OF_DAY, hour);
            cal.set(Calendar.MINUTE, minute);
            cal.set(Calendar.SECOND, 0);
            cal.set(Calendar.MILLISECOND, 0);
            if (cal.getTimeInMillis() <= now) { continue; }
            int iso = (cal.get(Calendar.DAY_OF_WEEK) + 5) % 7 + 1; // Calendar: domingo = 1 → ISO 7
            if (days == null || days.contains("," + iso + ",")) { return cal.getTimeInMillis(); }
        }
        throw new IllegalArgumentException("no next time for '" + trigger + "'");
    }

    static void schedule(Context c, String[] f) {
        if (!canExact(c)) {
            throw new IllegalStateException("exact alarms are not allowed for this app (Settings > Alarms & reminders)");
        }
        // Sin notificaciones no habría pantalla ni botones: la alarma sonaría sin forma de pararla.
        String st = RayNotify.status(c);
        if (!st.equals("granted")) {
            throw new IllegalStateException("alarms are not authorized for this app (notifications: " + st + "; call request_permission)");
        }
        String id = f[1];
        long at = next(f[0], System.currentTimeMillis());
        AlarmManager am = (AlarmManager) c.getSystemService(Context.ALARM_SERVICE);
        am.cancel(firePending(c, id + SNOOZE)); // una posposición en curso de la anterior
        prefs(c).edit().putString(id, at + "|" + String.join(SEP, f)).remove(id + SNOOZE).apply();
        arm(c, id, at);
    }

    static PendingIntent firePending(Context c, String key) {
        Intent i = new Intent(c, RayAlarmReceiver.class);
        i.setAction(ACTION_FIRE);
        i.setData(android.net.Uri.parse("rayalarm://" + android.net.Uri.encode(key))); // un PendingIntent por clave
        i.putExtra("key", key);
        return PendingIntent.getBroadcast(c, key.hashCode(), i, PendingIntent.FLAG_UPDATE_CURRENT | PendingIntent.FLAG_IMMUTABLE);
    }

    static void arm(Context c, String key, long at) {
        RayNotify.bootReceiver(c, true);
        AlarmManager am = (AlarmManager) c.getSystemService(Context.ALARM_SERVICE);
        PendingIntent show = PendingIntent.getActivity(c, 0, new Intent(c, MainActivity.class), PendingIntent.FLAG_IMMUTABLE);
        am.setAlarmClock(new AlarmManager.AlarmClockInfo(at, show), firePending(c, key));
    }

    static void cancel(Context c, String id) {
        AlarmManager am = (AlarmManager) c.getSystemService(Context.ALARM_SERVICE);
        am.cancel(firePending(c, id));
        am.cancel(firePending(c, id + SNOOZE));
        prefs(c).edit().remove(id).remove(id + SNOOZE).apply();
        RayAlarmService.stopIfRinging(c, id);
        RayNotify.syncBootReceiver(c);
    }

    // Un disparo (desde el receiver; `key` = el id, o `id#snooze` para una posposición): rearmar
    // la siguiente si repite y hacer sonar.
    static void fire(Context c, String key) {
        fire(c, key, true);
    }

    // `ring = false` (rearme tras reiniciar): la hora pasó con el teléfono apagado. Android 15 no
    // deja arrancar un servicio en primer plano desde BOOT_COMPLETED, así que no suena: se rearma
    // la siguiente y se informa como `missed`.
    static void fire(Context c, String key, boolean ring) {
        String rec = prefs(c).getString(key, null);
        if (rec == null) { return; }
        String[] f = rec.split("\\|", 2)[1].split(SEP, -1);
        if (!key.endsWith(SNOOZE) && !f[0].startsWith("at:")) {
            long at = next(f[0], System.currentTimeMillis());
            prefs(c).edit().putString(key, at + "|" + String.join(SEP, f)).apply();
            arm(c, key, at);
        } else {
            prefs(c).edit().remove(key).apply();
        }
        RayNotify.syncBootReceiver(c);
        if (!ring) {
            respond(c, f[1], "missed", f[6]);
            return;
        }
        try {
            RayAlarmService.ring(c, f);
        } catch (RuntimeException e) { // el sistema no dejó arrancar el servicio
            android.util.Log.w("ray", "alarm: could not start ringing: " + e);
            respond(c, f[1], "missed", f[6]);
        }
    }

    static String snoozeLabel(String[] f) {
        return f.length > 8 && !f[8].isEmpty() ? f[8] : "Snooze";
    }

    static void snooze(Context c, String[] f) {
        int minutes = 0;
        try { minutes = Integer.parseInt(f[4].trim()); } catch (NumberFormatException ignored) { }
        if (minutes <= 0) { return; }
        String key = f[1] + SNOOZE;
        long at = System.currentTimeMillis() + minutes * 60000L;
        prefs(c).edit().putString(key, at + "|" + String.join(SEP, f)).apply();
        arm(c, key, at);
    }

    // Tras reiniciar: lo vencido suena ya, lo demás se rearma.
    static void rearm(Context c) {
        if (!ENABLED) { return; }
        long now = System.currentTimeMillis();
        for (Map.Entry<String, ?> e : new java.util.HashMap<>(prefs(c).getAll()).entrySet()) {
            long at = Long.parseLong(String.valueOf(e.getValue()).split("\\|", 2)[0]);
            if (at <= now) { fire(c, e.getKey(), false); } else { arm(c, e.getKey(), at); }
        }
    }

    // Al abrir la app: repasar lo guardado, por si el aviso de arranque del sistema aún no llegó
    // (o no llegó nunca). Lo vencido hace más de un minuto se da por perdido; lo que venció hace
    // nada se deja a su disparo (o se arma para ya, si el reinicio lo desarmó); lo futuro se rearma.
    static void catchUp(Context c) {
        if (!ENABLED) { return; }
        long now = System.currentTimeMillis();
        for (Map.Entry<String, ?> e : new java.util.HashMap<>(prefs(c).getAll()).entrySet()) {
            long at = Long.parseLong(String.valueOf(e.getValue()).split("\\|", 2)[0]);
            if (at <= now - 60000) { fire(c, e.getKey(), false); } else { arm(c, e.getKey(), Math.max(at, now + 2000)); }
        }
    }

    // El reloj o la zona horaria cambiaron: una `daily`/`weekly` se armó como un instante
    // absoluto en la zona de entonces — se recalcula para que suene a su hora LOCAL.
    static void retime(Context c) {
        if (!ENABLED) { return; }
        long now = System.currentTimeMillis();
        for (Map.Entry<String, ?> e : new java.util.HashMap<>(prefs(c).getAll()).entrySet()) {
            if (e.getKey().endsWith(SNOOZE)) { continue; }
            String[] f = String.valueOf(e.getValue()).split("\\|", 2)[1].split(SEP, -1);
            if (f[0].startsWith("at:")) { continue; }
            long at = next(f[0], now);
            prefs(c).edit().putString(e.getKey(), at + "|" + String.join(SEP, f)).apply();
            arm(c, e.getKey(), at);
        }
    }

    // La respuesta del usuario → evento "alarm" (`id\taction\tdata`). Con el programa corriendo,
    // ya; si no (la alarma sonó con la app cerrada), se guarda para el siguiente arranque.
    static void respond(Context c, String id, String action, String data) {
        String tag = id + "\t" + action + "\t" + data;
        if (RayBridge.isStarted()) {
            RayBridge.pushEvent("alarm", 0, tag);
        } else {
            SharedPreferences p = c.getSharedPreferences(RESPONSES, Context.MODE_PRIVATE);
            p.edit().putString(String.format(java.util.Locale.ROOT, "%020d-%03d", System.currentTimeMillis(), p.getAll().size()), tag).apply();
        }
    }

    // Al arrancar el programa (MainActivity.onCreate, tras startOnce): las respuestas guardadas, en orden.
    static void deliverPending(Context c) {
        SharedPreferences p = c.getSharedPreferences(RESPONSES, Context.MODE_PRIVATE);
        if (p.getAll().isEmpty()) { return; }
        for (Map.Entry<String, ?> e : new TreeMap<>(p.getAll()).entrySet()) {
            RayBridge.pushEvent("alarm", 0, String.valueOf(e.getValue()));
        }
        p.edit().clear().apply();
    }
}
"#;

/// M448: la alarma vencida (un `setAlarmClock` de `RayAlarm`).
const RAY_ALARM_RECEIVER_JAVA: &str = r#"package org.raylang.shell;

import android.content.BroadcastReceiver;
import android.content.Context;
import android.content.Intent;

public class RayAlarmReceiver extends BroadcastReceiver {
    @Override
    public void onReceive(Context c, Intent intent) {
        if (RayAlarm.ACTION_FIRE.equals(intent.getAction())) {
            String key = intent.getStringExtra("key");
            if (key != null) { RayAlarm.fire(c, key); }
        }
    }
}
"#;

/// M448: el servicio que SUENA. Primer plano (`mediaPlayback`), `MediaPlayer` en bucle con
/// `USAGE_ALARM` (el volumen de alarma: suena con el teléfono en silencio), notificación
/// `CATEGORY_ALARM` con full-screen intent y los botones; se calla al responder, al cancelar la
/// alarma o a los `max_minutes` (60 como tope).
const RAY_ALARM_SERVICE_JAVA: &str = r#"package org.raylang.shell;

import android.app.Notification;
import android.app.NotificationChannel;
import android.app.NotificationManager;
import android.app.PendingIntent;
import android.app.Service;
import android.content.Context;
import android.content.Intent;
import android.content.pm.ServiceInfo;
import android.media.AudioAttributes;
import android.media.MediaPlayer;
import android.media.RingtoneManager;
import android.net.Uri;
import android.os.Build;
import android.os.Handler;
import android.os.IBinder;
import android.os.Looper;

public class RayAlarmService extends Service {
    static final String CHANNEL = "ray_alarm";
    static final int NOTIFICATION_ID = 7448;
    static String[] current = null; // la alarma que está sonando (campos de RayAlarm)
    private MediaPlayer player;
    private final Handler handler = new Handler(Looper.getMainLooper());
    private final Runnable timeout = () -> finishWith("timeout");

    static void ring(Context c, String[] f) {
        Intent i = new Intent(c, RayAlarmService.class);
        i.setAction(RayAlarm.ACTION_FIRE);
        i.putExtra("rec", String.join(RayAlarm.SEP, f));
        if (Build.VERSION.SDK_INT >= 26) { c.startForegroundService(i); } else { c.startService(i); }
    }

    static void stopAny(Context c) {
        if (current != null) { c.stopService(new Intent(c, RayAlarmService.class)); }
    }

    static void stopIfRinging(Context c, String id) {
        if (current != null && current[1].equals(id)) {
            c.stopService(new Intent(c, RayAlarmService.class));
        }
    }

    @Override
    public int onStartCommand(Intent intent, int flags, int startId) {
        String action = intent != null ? intent.getAction() : null;
        if (RayAlarm.ACTION_FIRE.equals(action)) {
            begin(intent.getStringExtra("rec").split(RayAlarm.SEP, -1));
        } else if (RayAlarm.ACTION_STOP.equals(action)) {
            finishWith("stop");
        } else if (RayAlarm.ACTION_SNOOZE.equals(action)) {
            finishWith("snooze");
        } else if (current == null) {
            stopSelf();
        }
        return START_NOT_STICKY;
    }

    private PendingIntent self(String action) {
        Intent i = new Intent(this, RayAlarmService.class);
        i.setAction(action);
        return PendingIntent.getService(this, action.hashCode(), i, PendingIntent.FLAG_UPDATE_CURRENT | PendingIntent.FLAG_IMMUTABLE);
    }

    private void begin(String[] f) {
        // Otra que estuviera sonando cede: se informa como `timeout` (nadie la respondió) y su
        // pantalla se cierra; la nueva abre la suya.
        String[] old = current;
        silence();
        if (old != null && !old[1].equals(f[1])) {
            RayAlarm.respond(this, old[1], "timeout", old[6]);
            sendBroadcast(new Intent(RayAlarm.ACTION_DONE).setPackage(getPackageName()));
        }
        current = f;
        int snooze = 0;
        int max = 0;
        try { snooze = Integer.parseInt(f[4].trim()); } catch (NumberFormatException ignored) { }
        try { max = Integer.parseInt(f[7].trim()); } catch (NumberFormatException ignored) { }
        String stop = f[5].isEmpty() ? "Stop" : f[5];
        NotificationManager nm = (NotificationManager) getSystemService(Context.NOTIFICATION_SERVICE);
        Notification.Builder b;
        if (Build.VERSION.SDK_INT >= 26) {
            // Canal propio y mudo: el sonido lo pone el MediaPlayer con el volumen de alarma.
            NotificationChannel ch = new NotificationChannel(CHANNEL, "Alarms", NotificationManager.IMPORTANCE_HIGH);
            ch.setSound(null, null);
            ch.setLockscreenVisibility(Notification.VISIBILITY_PUBLIC);
            nm.createNotificationChannel(ch);
            b = new Notification.Builder(this, CHANNEL);
        } else {
            b = new Notification.Builder(this);
        }
        Intent screen = new Intent(this, RayAlarmActivity.class);
        screen.setFlags(Intent.FLAG_ACTIVITY_NEW_TASK | Intent.FLAG_ACTIVITY_NO_USER_ACTION);
        PendingIntent full = PendingIntent.getActivity(this, 0, screen, PendingIntent.FLAG_UPDATE_CURRENT | PendingIntent.FLAG_IMMUTABLE);
        android.graphics.drawable.Icon icon = android.graphics.drawable.Icon.createWithResource(this, android.R.drawable.ic_lock_idle_alarm);
        b.setContentTitle(f[2])
            .setSmallIcon(android.R.drawable.ic_lock_idle_alarm)
            .setCategory(Notification.CATEGORY_ALARM)
            .setVisibility(Notification.VISIBILITY_PUBLIC)
            .setOngoing(true)
            .setContentIntent(full)
            .setFullScreenIntent(full, true)
            .addAction(new Notification.Action.Builder(icon, stop, self(RayAlarm.ACTION_STOP)).build());
        if (snooze > 0) {
            b.addAction(new Notification.Action.Builder(icon, RayAlarm.snoozeLabel(f), self(RayAlarm.ACTION_SNOOZE)).build());
        }
        if (Build.VERSION.SDK_INT >= 34 && !nm.canUseFullScreenIntent()) {
            android.util.Log.w("ray", "alarm: full-screen alarms are not allowed for this app (Settings > Apps > Special app access > Full screen intents); it rings with a notification");
        }
        Notification n = b.build();
        if (Build.VERSION.SDK_INT >= 29) {
            startForeground(NOTIFICATION_ID, n, ServiceInfo.FOREGROUND_SERVICE_TYPE_MEDIA_PLAYBACK);
        } else {
            startForeground(NOTIFICATION_ID, n);
        }
        play(f[3]);
        handler.postDelayed(timeout, (max > 0 ? max : 60) * 60000L);
    }

    private void play(String sound) {
        try {
            Uri uri = RayNotify.soundUri(this, sound);
            if (uri == null) { uri = RingtoneManager.getDefaultUri(RingtoneManager.TYPE_ALARM); }
            if (uri == null) { uri = RingtoneManager.getDefaultUri(RingtoneManager.TYPE_RINGTONE); }
            if (uri == null) { return; }
            player = new MediaPlayer();
            player.setAudioAttributes(new AudioAttributes.Builder()
                .setUsage(AudioAttributes.USAGE_ALARM)
                .setContentType(AudioAttributes.CONTENT_TYPE_SONIFICATION).build());
            player.setDataSource(this, uri);
            player.setLooping(true);
            player.prepare();
            player.start();
        } catch (Exception e) {
            android.util.Log.w("ray", "alarm: could not play the sound: " + e);
        }
    }

    private void silence() {
        handler.removeCallbacks(timeout);
        if (player != null) {
            try { player.stop(); } catch (IllegalStateException ignored) { }
            player.release();
            player = null;
        }
    }

    private void finishWith(String action) {
        String[] f = current;
        silence();
        current = null;
        if (f != null) {
            if (action.equals("snooze")) { RayAlarm.snooze(this, f); }
            RayAlarm.respond(this, f[1], action, f[6]);
        }
        stopForeground(Service.STOP_FOREGROUND_REMOVE);
        stopSelf();
    }

    @Override
    public void onDestroy() {
        silence();
        current = null;
        sendBroadcast(new Intent(RayAlarm.ACTION_DONE).setPackage(getPackageName()));
        super.onDestroy();
    }

    @Override
    public IBinder onBind(Intent intent) {
        return null;
    }
}
"#;

/// M448: la pantalla de la alarma, sobre el bloqueo (full-screen intent de `RayAlarmService`):
/// el título y los botones; se cierra sola cuando la alarma deja de sonar.
const RAY_ALARM_ACTIVITY_JAVA: &str = r#"package org.raylang.shell;

import android.app.Activity;
import android.content.BroadcastReceiver;
import android.content.Context;
import android.content.Intent;
import android.content.IntentFilter;
import android.os.Build;
import android.os.Bundle;
import android.view.Gravity;
import android.view.WindowManager;
import android.widget.Button;
import android.widget.LinearLayout;
import android.widget.TextView;

public class RayAlarmActivity extends Activity {
    private BroadcastReceiver done;

    @Override
    @SuppressWarnings("deprecation")
    protected void onCreate(Bundle savedInstanceState) {
        super.onCreate(savedInstanceState);
        if (Build.VERSION.SDK_INT >= 27) {
            setShowWhenLocked(true);
            setTurnScreenOn(true);
        } else {
            getWindow().addFlags(WindowManager.LayoutParams.FLAG_SHOW_WHEN_LOCKED | WindowManager.LayoutParams.FLAG_TURN_SCREEN_ON);
        }
        getWindow().addFlags(WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON);
        String[] f = RayAlarmService.current;
        if (f == null) { // la alarma ya no suena (respondida desde la notificación)
            finish();
            return;
        }
        int snooze = 0;
        try { snooze = Integer.parseInt(f[4].trim()); } catch (NumberFormatException ignored) { }
        float d = getResources().getDisplayMetrics().density;
        LinearLayout box = new LinearLayout(this);
        box.setOrientation(LinearLayout.VERTICAL);
        box.setGravity(Gravity.CENTER);
        box.setPadding((int) (32 * d), (int) (48 * d), (int) (32 * d), (int) (48 * d));
        TextView title = new TextView(this);
        title.setText(f[2]);
        title.setTextSize(28);
        title.setGravity(Gravity.CENTER);
        box.addView(title, new LinearLayout.LayoutParams(LinearLayout.LayoutParams.MATCH_PARENT, 0, 1));
        Button stop = new Button(this);
        stop.setText(f[5].isEmpty() ? "Stop" : f[5]);
        stop.setTextSize(20);
        stop.setAllCaps(false);
        stop.setOnClickListener(v -> answer(RayAlarm.ACTION_STOP));
        LinearLayout.LayoutParams lp = new LinearLayout.LayoutParams(LinearLayout.LayoutParams.MATCH_PARENT, (int) (64 * d));
        lp.topMargin = (int) (12 * d);
        box.addView(stop, lp);
        if (snooze > 0) {
            Button later = new Button(this);
            later.setText(RayAlarm.snoozeLabel(f) + " (" + snooze + " min)");
            later.setTextSize(18);
            later.setAllCaps(false);
            later.setOnClickListener(v -> answer(RayAlarm.ACTION_SNOOZE));
            LinearLayout.LayoutParams lp2 = new LinearLayout.LayoutParams(LinearLayout.LayoutParams.MATCH_PARENT, (int) (56 * d));
            lp2.topMargin = (int) (12 * d);
            box.addView(later, lp2);
        }
        setContentView(box);
        done = new BroadcastReceiver() {
            @Override
            public void onReceive(Context c, Intent i) { finish(); }
        };
        IntentFilter filter = new IntentFilter(RayAlarm.ACTION_DONE);
        if (Build.VERSION.SDK_INT >= 33) {
            registerReceiver(done, filter, Context.RECEIVER_NOT_EXPORTED);
        } else {
            registerReceiver(done, filter);
        }
    }

    private void answer(String action) {
        startService(new Intent(this, RayAlarmService.class).setAction(action));
        finish();
    }

    @Override
    protected void onNewIntent(Intent intent) {
        super.onNewIntent(intent);
        recreate(); // otra alarma tomó el relevo: su título y sus botones
    }

    @Override
    protected void onDestroy() {
        if (done != null) { unregisterReceiver(done); }
        super.onDestroy();
    }
}
"#;

/// M442: el receiver de las notificaciones programadas (alarma vencida) y del reinicio.
const RAY_NOTIFY_RECEIVER_JAVA: &str = r#"package org.raylang.shell;

import android.content.BroadcastReceiver;
import android.content.Context;
import android.content.Intent;

public class RayNotifyReceiver extends BroadcastReceiver {
    @Override
    public void onReceive(Context c, Intent intent) {
        String action = intent.getAction();
        if (Intent.ACTION_BOOT_COMPLETED.equals(action)) {
            RayNotify.rearm(c);
            RayAlarm.rearm(c); // M448: las alarmas de std/alarm
        } else if (Intent.ACTION_TIMEZONE_CHANGED.equals(action) || Intent.ACTION_TIME_CHANGED.equals(action)) {
            RayAlarm.retime(c); // M448: las diarias/semanales siguen la hora local
        } else if (RayNotify.ACTION_FIRE.equals(action)) {
            String id = intent.getStringExtra("id");
            if (id != null) { RayNotify.fire(c, id); }
        }
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
    // WebViewCompat.addDocumentStartJavaScript (el shim window.ray antes de la página).
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
- El puente IPC funciona igual que en escritorio/iOS: `window.ray.send(text)` llega
  como evento `"message"` (window 0). Los eventos `lifecycle` llegan en onPause/onResume.
- `ray://app/…`: `ui.open` con una URL `ray://app/…` la sirve desde el programa
  (`ui.mount_embed`/`mount_dir`/`mount_bytes`), sin puerto. El WebView la carga por el alias
  `https://app.ray.invalid/…` (Chromium no admite `fetch()` hacia un esquema propio); las
  rutas relativas y `fetch("/api/x")` de la página funcionan igual, y un enlace absoluto
  `ray://app/…` se reescribe al alias al navegar.
- `std/fs`/`std/kv`: escribe en el directorio privado de la app (el cwd no es tuyo); las
  rutas externas están restringidas (scoped storage) — también para `fs.watch`.
- Firma: el debug keystore de Gradle basta para instalar. **Release**: crea un
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
- Icono: `ray bundle --android --icon icon.png` genera los `mipmap-*/ic_launcher.png`
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
            super::write_project(&base, "App", "org.example.app", "1.0.0", "arm64-v8a", false, devtools, false, None, false, false).unwrap();
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
    url_scheme: Option<&str>,
    exact_alarms: bool,
    alarm: bool,
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
    write("app/src/main/AndroidManifest.xml", &android_manifest(icon, background_audio, url_scheme, exact_alarms, alarm))?;
    write("app/src/main/res/xml/network_security_config.xml", NETWORK_SECURITY_XML)?;
    write("app/src/main/res/values/strings.xml", &strings_xml(name))?;
    write("app/src/main/res/values/styles.xml", STYLES_XML)?;
    write("app/src/main/res/values-night/styles.xml", STYLES_NIGHT_XML)?;
    // M231: `--devtools` → depuración remota desde chrome://inspect (Chrome del escritorio).
    let devtools_line = if devtools { "WebView.setWebContentsDebuggingEnabled(true); // ray bundle --devtools" } else { "" };
    // M324 (ray808 #18): el flag de audio en segundo plano va como constante de la clase.
    let bg = if background_audio { "true" } else { "false" };
    // M330 D5: el shell de desarrollo entrega a la librería las URLs de su esquema (el QR).
    // (M442: `onNewIntent` existe siempre —las notificaciones pulsadas llegan por ahí— y el shell de
    // desarrollo solo añade su llamada dentro.)
    let (dev_oncreate, dev_newintent, dev_method, dev_native) = match url_scheme {
        Some(scheme) => (
            format!("rayDevLink(getIntent()); // Enlace recibido al arrancar ({scheme}://…)"),
            "rayDevLink(intent); // La app ya abierta recibe un enlace (QR escaneado con la cámara del sistema)".to_string(),
            format!(
                "private static void rayDevLink(Intent intent) {{\n        android.net.Uri data = intent != null ? intent.getData() : null;\n        if (data != null && \"{scheme}\".equals(data.getScheme())) {{\n            RayBridge.devLink(data.toString());\n        }}\n    }}"
            ),
            "public static native void devLink(String url); // Solo la librería de desarrollo lo define".to_string(),
        ),
        None => (String::new(), String::new(), String::new(), String::new()),
    };
    let main = MAIN_ACTIVITY_JAVA
        .replace("/*RAY_DEVTOOLS*/", devtools_line)
        .replace("/*RAY_BACKGROUND_AUDIO*/false", bg)
        .replace("/*RAY_DEV_LINK_ONCREATE*/", &dev_oncreate)
        .replace("/*RAY_DEV_LINK_NEWINTENT*/", &dev_newintent)
        .replace("/*RAY_DEV_LINK_METHOD*/", &dev_method);
    write("app/src/main/java/org/raylang/shell/MainActivity.java", &main)?;
    // M325 (findings #99): siempre — MainActivity la referencia (tras la constante) y sin la clase
    // `gradle assembleDebug` fallaba con «cannot find symbol: variable RayPlaybackService».
    write("app/src/main/java/org/raylang/shell/RayPlaybackService.java", RAY_PLAYBACK_SERVICE_JAVA)?;
    write("app/src/main/java/org/raylang/shell/RayBridge.java", &RAY_BRIDGE_JAVA.replace("/*RAY_DEV_NATIVE*/", &dev_native))?;
    write("app/src/main/java/org/raylang/shell/RayScheme.java", RAY_SCHEME_JAVA)?;
    write("app/src/main/java/org/raylang/shell/RayMedia.java", RAY_MEDIA_JAVA)?;
    // M442 (IDEAS §121): std/notify — siempre (MainActivity y RayBridge las referencian); la
    // constante decide las alarmas exactas.
    let exact = if exact_alarms { "true" } else { "false" };
    write("app/src/main/java/org/raylang/shell/RayNotify.java", &RAY_NOTIFY_JAVA.replace("/*RAY_EXACT_ALARMS*/false", exact))?;
    write("app/src/main/java/org/raylang/shell/RayNotifyReceiver.java", RAY_NOTIFY_RECEIVER_JAVA)?;
    // M448 (IDEAS §122): std/alarm — las clases siempre (RayBridge/MainActivity las referencian);
    // la constante y el manifiesto solo las activan si el programa importa std/alarm.
    write("app/src/main/java/org/raylang/shell/RayAlarm.java", &RAY_ALARM_JAVA.replace("/*RAY_ALARM*/false", if alarm { "true" } else { "false" }))?;
    write("app/src/main/java/org/raylang/shell/RayAlarmReceiver.java", RAY_ALARM_RECEIVER_JAVA)?;
    write("app/src/main/java/org/raylang/shell/RayAlarmService.java", RAY_ALARM_SERVICE_JAVA)?;
    write("app/src/main/java/org/raylang/shell/RayAlarmActivity.java", RAY_ALARM_ACTIVITY_JAVA)?;
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
        // M382: el selector de archivos del sistema (onPick → MainActivity.rayPick → pickResult).
        assert!(RAY_BRIDGE_JAVA.contains("public static native void pickResult(long id, String paths, String err)"));
        assert!(RAY_BRIDGE_JAVA.contains("public static void onPick(String spec)"));
        assert!(MAIN_ACTIVITY_JAVA.contains("Intent.ACTION_OPEN_DOCUMENT") && MAIN_ACTIVITY_JAVA.contains("requestCode == RAY_PICK"));
        assert!(RAY_BRIDGE_JAVA.contains("System.loadLibrary(\"ray_app\")"));
        assert!(RAY_BRIDGE_JAVA.contains("public static native int start()"));
        assert!(RAY_BRIDGE_JAVA
            .contains("public static native void pushEvent(String kind, long window, String tag)"));
        assert!(MAIN_ACTIVITY_JAVA.contains("window.ray={send:function(t){q(e(t))}"));
        assert!(MAIN_ACTIVITY_JAVA.contains("Os.setenv(\"RAY_DEV_APP_NAME\", getApplicationInfo().loadLabel(getPackageManager()).toString(), true)"), "M343: the app label reaches the dev library");
        assert!(MAIN_ACTIVITY_JAVA.contains("request:function(t)"), "M157: request in the shim");
        // M323 (ray808 #12): el shim como script de inicio de documento, con fallback a onPageStarted.
        assert!(MAIN_ACTIVITY_JAVA.contains("WebViewCompat.addDocumentStartJavaScript(web, RAY_SHIM, Collections.singleton(\"*\"))"));
        assert!(MAIN_ACTIVITY_JAVA.contains("if (!shimInjected) {\n                    v.evaluateJavascript(RAY_SHIM, null);"));
        assert!(app_build_gradle("org.raylang.demo", "1.0.0", "'arm64-v8a'").contains("implementation 'androidx.webkit:webkit:1.12.1'"));
        assert!(android_manifest(false, false, None, false, false)
            .contains("android:networkSecurityConfig=\"@xml/network_security_config\""));
        assert!(NETWORK_SECURITY_XML.contains("127.0.0.1"));
        assert!(!android_manifest(false, false, None, false, false).contains("usesCleartextTraffic"));
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
        assert!(RAY_BRIDGE_JAVA.contains("capabilities(1); // Este shell sirve ray://app"));
        assert!(RAY_BRIDGE_JAVA.contains("public static native void capabilities(int caps)"));
        assert!(MAIN_ACTIVITY_JAVA.contains("public WebResourceResponse shouldInterceptRequest(WebView v, WebResourceRequest req)"));
        assert!(MAIN_ACTIVITY_JAVA.contains("return RayScheme.intercept(req);"));
        assert!(RAY_SCHEME_JAVA.contains("static final String ALIAS_HOST = \"app.ray.invalid\";"));
        assert!(RAY_SCHEME_JAVA.contains("(\"ray\".equals(scheme) && \"app\".equals(host))"));
        // If-None-Match nunca viaja: WebResourceResponse no admite 304.
        assert!(RAY_SCHEME_JAVA.contains("RayBridge.schemeOpen(url, method == null ? \"GET\" : method, range, null)"));
        assert!(!android_manifest(false, false, None, false, false).contains("app.ray.invalid"), "the alias needs no manifest entry");
    }

    #[test]
    fn the_manifest_only_declares_the_icon_when_the_mipmaps_exist() {
        // M160: el atributo sin los PNG rompe aapt — solo con icon=true.
        assert!(android_manifest(true, false, None, false, false).contains("android:icon=\"@mipmap/ic_launcher\""));
        assert!(!android_manifest(false, false, None, false, false).contains("android:icon"));
    }

    /// M334: con targetSdk 35 Android dibuja de borde a borde; el shell reserva las barras del
    /// sistema, el recorte de la cámara y el teclado como relleno de un FrameLayout alrededor del
    /// WebView (si no, la página queda bajo la barra de estado y el teclado tapa los campos).
    #[test]
    fn the_shell_pads_the_webview_with_the_system_insets() {
        assert!(MAIN_ACTIVITY_JAVA.contains("setContentView(frame);"));
        assert!(!MAIN_ACTIVITY_JAVA.contains("setContentView(web);"));
        for kind in ["systemBars()", "displayCutout()", "ime()"] {
            assert!(MAIN_ACTIVITY_JAVA.contains(&format!("WindowInsets.Type.{kind}")), "{kind}");
        }
        assert!(MAIN_ACTIVITY_JAVA.contains("Build.VERSION.SDK_INT >= 30"), "la API de insets es de la 30");
    }

    /// rayplay #177: el tema sigue al sistema (claro/oscuro) y la Activity no se recrea al
    /// cambiar el modo; en API 29–32 el WebView recibe force dark «solo tema web».
    #[test]
    fn the_shell_theme_follows_the_system_dark_mode() {
        let manifest = android_manifest(false, false, None, false, false);
        assert!(manifest.contains("android:theme=\"@style/RayTheme\""));
        assert!(!manifest.contains("Theme.Material.Light.NoActionBar"), "el tema claro fijo era la causa de #177");
        assert!(manifest.contains("|uiMode\""), "sin uiMode en configChanges el cambio de modo recrea la Activity y recarga la página");
        assert!(STYLES_XML.contains("Theme.Material.Light.NoActionBar"));
        assert!(STYLES_NIGHT_XML.contains("Theme.Material.NoActionBar"));
        assert!(!STYLES_NIGHT_XML.contains("Theme.Material.Light"));
        assert!(MAIN_ACTIVITY_JAVA.contains("rayApplyTheme(getResources().getConfiguration());"));
        // rayplay #181: iconos de las barras del sistema según el modo, al arrancar y al cambiar.
        assert!(STYLES_XML.contains("android:windowLightStatusBar\">true"));
        assert!(STYLES_NIGHT_XML.contains("android:windowLightStatusBar\">false"));
        assert!(MAIN_ACTIVITY_JAVA.contains("APPEARANCE_LIGHT_STATUS_BARS"));
        assert!(MAIN_ACTIVITY_JAVA.contains("SYSTEM_UI_FLAG_LIGHT_STATUS_BAR"), "API 24–29");
        assert!(MAIN_ACTIVITY_JAVA.contains("rayApplyTheme(newConfig);"));
        assert!(MAIN_ACTIVITY_JAVA.contains("public void onConfigurationChanged(Configuration newConfig)"));
        assert!(MAIN_ACTIVITY_JAVA.contains("DARK_STRATEGY_WEB_THEME_DARKENING_ONLY"));
        assert!(MAIN_ACTIVITY_JAVA.contains("SDK_INT >= 33) { return; }"), "desde la 33 manda isLightTheme del tema");
    }

    /// M404 (rayplay #170): el shell trae `RayMedia.java` (MediaSession + notificación MediaStyle)
    /// y `RayBridge.onMedia`, el camino por el que `std/media` entra al shell.
    #[test]
    fn the_shell_ships_the_media_session_bridge() {
        assert!(RAY_BRIDGE_JAVA.contains("public static void onMedia(String op, String arg)"));
        // rayreq #263: `ui.open_url` entra al shell por `onOpenUrl` (Intent ACTION_VIEW).
        assert!(RAY_BRIDGE_JAVA.contains("public static void onOpenUrl(String url)"));
        // M445 (pepa #331): `ui.haptic` entra por `onHaptic` (HapticFeedbackConstants sobre la vista).
        assert!(RAY_BRIDGE_JAVA.contains("public static void onHaptic(String kind)"));
        assert!(RAY_BRIDGE_JAVA.contains("v.performHapticFeedback(c)"));
        assert!(RAY_BRIDGE_JAVA.contains("RayMedia.apply(op, arg)"));
        assert!(RAY_MEDIA_JAVA.contains("new MediaSession(c, \"ray\")"));
        assert!(RAY_MEDIA_JAVA.contains("Notification.MediaStyle().setMediaSession"));
        for tag in ["\"play\"", "\"pause\"", "\"next\"", "\"previous\"", "\"stop\"", "\"seek:\" + pos"] {
            assert!(RAY_MEDIA_JAVA.contains(&format!("RayBridge.pushEvent(\"media\", 0, {tag})")) || RAY_MEDIA_JAVA.contains(tag), "{tag}");
        }
        for op in ["media_controls", "media_now_playing", "media_clear"] {
            assert!(RAY_MEDIA_JAVA.contains(&format!("case \"{op}\"")), "{op}");
        }
    }

    /// M324 (ray808 #18): `[android] background_audio` declara el servicio de reproducción con sus
    /// permisos, escribe la clase y enciende el flag de MainActivity; sin él, nada de eso existe.
    #[test]
    fn background_audio_adds_the_foreground_service_only_when_asked() {
        let with = android_manifest(false, true, None, false, false);
        assert!(with.contains("android:name=\".RayPlaybackService\""), "{with}");
        assert!(with.contains("android:foregroundServiceType=\"mediaPlayback\""), "{with}");
        for perm in ["FOREGROUND_SERVICE", "FOREGROUND_SERVICE_MEDIA_PLAYBACK", "POST_NOTIFICATIONS"] {
            assert!(with.contains(&format!("android.permission.{perm}")), "{perm}");
        }
        // M442: POST_NOTIFICATIONS lo declara siempre std/notify (se pide en tiempo de ejecución).
        assert!(android_manifest(false, false, None, false, false).contains("android.permission.POST_NOTIFICATIONS"));
        let without = android_manifest(false, false, None, false, false);
        assert!(!without.contains("RayPlaybackService") && !without.contains("FOREGROUND_SERVICE"), "{without}");
        assert!(MAIN_ACTIVITY_JAVA.contains("if (RAY_BACKGROUND_AUDIO) { RayPlaybackService.start(this); }"));
        assert!(RAY_PLAYBACK_SERVICE_JAVA.contains("ServiceInfo.FOREGROUND_SERVICE_TYPE_MEDIA_PLAYBACK"));
        let base = std::env::temp_dir().join(format!("ray_android_audio_{}", std::process::id()));
        for (bg, want) in [(true, true), (false, false)] {
            let _ = std::fs::remove_dir_all(&base);
            write_project(&base, "App", "org.example.app", "1.0.0", "'arm64-v8a'", false, false, bg, None, false, false).unwrap();
            let main = std::fs::read_to_string(base.join("app/src/main/java/org/raylang/shell/MainActivity.java")).unwrap();
            assert_eq!(main.contains("RAY_BACKGROUND_AUDIO = true;"), want, "bg={bg}");
            assert!(!main.contains("/*RAY_BACKGROUND_AUDIO*/"), "sin marcador");
            // M325 (findings #99): la clase existe SIEMPRE (MainActivity la referencia).
            assert!(base.join("app/src/main/java/org/raylang/shell/RayPlaybackService.java").is_file(), "bg={bg}");
            assert!(!main.contains("requestPermissions"), "sin diálogo de permisos al arrancar (findings #99)");
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

    /// M330 D5: el shell de desarrollo abre las URLs de su esquema (el QR del enlace) y las
    /// entrega a la librería; el shell normal no lleva nada de eso.
    #[test]
    fn the_dev_shell_registers_its_url_scheme_and_hands_links_to_the_library() {
        let with = android_manifest(false, false, Some("org.example.app.dev"), false, false);
        assert!(with.contains("<data android:scheme=\"org.example.app.dev\" />"));
        assert!(with.contains("android.intent.category.BROWSABLE"));
        assert!(with.contains("android:launchMode=\"singleTask\""));
        assert!(!android_manifest(false, false, None, false, false).contains("launchMode"));
        assert!(!android_manifest(false, false, None, false, false).contains("android.intent.action.VIEW"));
        let base = std::env::temp_dir().join(format!("ray-android-devlink-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&base).unwrap();
        write_project(&base, "App", "org.example.app.dev", "1.0.0", "'arm64-v8a'", false, true, false, Some("org.example.app.dev"), false, false).unwrap();
        let main = std::fs::read_to_string(base.join("app/src/main/java/org/raylang/shell/MainActivity.java")).unwrap();
        assert!(main.contains("rayDevLink(getIntent());"));
        assert!(main.contains("protected void onNewIntent(Intent intent)"));
        assert!(main.contains("\"org.example.app.dev\".equals(data.getScheme())"));
        let bridge = std::fs::read_to_string(base.join("app/src/main/java/org/raylang/shell/RayBridge.java")).unwrap();
        assert!(bridge.contains("public static native void devLink(String url);"));
        write_project(&base, "App", "org.example.app", "1.0.0", "'arm64-v8a'", false, false, false, None, false, false).unwrap();
        let main = std::fs::read_to_string(base.join("app/src/main/java/org/raylang/shell/MainActivity.java")).unwrap();
        assert!(!main.contains("rayDevLink"));
        assert!(main.contains("protected void onNewIntent(Intent intent)"), "M442: onNewIntent existe siempre");
        let _ = std::fs::remove_dir_all(&base);
    }

    /// M448 (IDEAS §122): std/alarm — las clases van siempre, pero los permisos de alarma, el
    /// servicio, la pantalla y el receiver solo entran al manifiesto (y `ENABLED` solo es true)
    /// cuando el programa importa `std/alarm`.
    #[test]
    fn the_alarm_components_are_declared_only_when_the_program_uses_std_alarm() {
        let with = android_manifest(false, false, None, false, true);
        for needle in [
            "android.permission.USE_EXACT_ALARM",
            "android.permission.SCHEDULE_EXACT_ALARM\" android:maxSdkVersion=\"32\"",
            "android.permission.USE_FULL_SCREEN_INTENT",
            "android.permission.FOREGROUND_SERVICE_MEDIA_PLAYBACK",
            "android:name=\".RayAlarmActivity\"",
            "android:showWhenLocked=\"true\"",
            "android:name=\".RayAlarmService\"",
            "android:name=\".RayAlarmReceiver\"",
        ] {
            assert!(with.contains(needle), "{needle}\n{with}");
        }
        let without = android_manifest(false, false, None, false, false);
        for needle in ["USE_EXACT_ALARM", "USE_FULL_SCREEN_INTENT", "RayAlarm"] {
            assert!(!without.contains(needle), "{needle}");
        }
        // Con background_audio los permisos del servicio no se duplican; con exact_alarms, SCHEDULE sin tope.
        let both = android_manifest(false, true, None, true, true);
        assert_eq!(both.matches("android.permission.FOREGROUND_SERVICE\"").count(), 1, "{both}");
        assert_eq!(both.matches("android.permission.SCHEDULE_EXACT_ALARM").count(), 1, "{both}");
        assert!(!both.contains("maxSdkVersion"), "{both}");
        let base = std::env::temp_dir().join(format!("ray_android_alarm_{}", std::process::id()));
        for alarm in [true, false] {
            let _ = std::fs::remove_dir_all(&base);
            write_project(&base, "App", "org.example.app", "1.0.0", "'arm64-v8a'", false, false, false, None, false, alarm).unwrap();
            let java = std::fs::read_to_string(base.join("app/src/main/java/org/raylang/shell/RayAlarm.java")).unwrap();
            assert_eq!(java.contains("static final boolean ENABLED = true;"), alarm);
            for f in ["RayAlarmReceiver.java", "RayAlarmService.java", "RayAlarmActivity.java"] {
                assert!(base.join("app/src/main/java/org/raylang/shell").join(f).is_file(), "{f}");
            }
        }
        let _ = std::fs::remove_dir_all(&base);
        // El contrato de las piezas: alarma de reloj, volumen de alarma, pantalla completa, respuesta.
        assert!(RAY_ALARM_JAVA.contains("am.setAlarmClock(new AlarmManager.AlarmClockInfo(at, show), firePending(c, key));"));
        assert!(RAY_ALARM_JAVA.contains("RayBridge.pushEvent(\"alarm\", 0, tag);") && RAY_ALARM_JAVA.contains("static void deliverPending(Context c)"));
        assert!(RAY_ALARM_SERVICE_JAVA.contains(".setUsage(AudioAttributes.USAGE_ALARM)") && RAY_ALARM_SERVICE_JAVA.contains(".setFullScreenIntent(full, true)"));
        assert!(RAY_ALARM_SERVICE_JAVA.contains("ServiceInfo.FOREGROUND_SERVICE_TYPE_MEDIA_PLAYBACK") && RAY_ALARM_SERVICE_JAVA.contains("finishWith(\"timeout\")"));
        assert!(RAY_ALARM_ACTIVITY_JAVA.contains("setShowWhenLocked(true);") && RAY_ALARM_ACTIVITY_JAVA.contains("Context.RECEIVER_NOT_EXPORTED"));
        // Revisión 1.27.93: sin sonar desde BOOT_COMPLETED (Android 15), sin armar sin permiso de
        // notificaciones, las pospuestas cuentan como pendientes y las diarias siguen la hora local.
        assert!(RAY_ALARM_JAVA.contains("fire(c, e.getKey(), false)") && RAY_ALARM_JAVA.contains("respond(c, f[1], \"missed\", f[6]);"));
        assert!(RAY_ALARM_JAVA.contains("alarms are not authorized for this app (notifications: ") && RAY_ALARM_JAVA.contains("static void retime(Context c)"));
        assert!(RAY_ALARM_JAVA.contains("k.endsWith(SNOOZE) ? k.substring(0, k.length() - SNOOZE.length()) : k"));
        assert!(with.contains("android.intent.action.TIMEZONE_CHANGED") && RAY_NOTIFY_RECEIVER_JAVA.contains("RayAlarm.retime(c);"));
        assert!(RAY_BRIDGE_JAVA.contains("if (op.startsWith(\"alarm_\")) {") && RAY_BRIDGE_JAVA.contains("static boolean isStarted()"));
        let (catch_up, deliver) = (MAIN_ACTIVITY_JAVA.find("RayAlarm.catchUp(getApplicationContext());").unwrap(), MAIN_ACTIVITY_JAVA.find("RayAlarm.deliverPending(getApplicationContext());").unwrap());
        assert!(catch_up < deliver, "el repaso de lo guardado va antes de entregar las respuestas");
        assert!(RAY_ALARM_JAVA.contains("if (at <= now - 60000) { fire(c, e.getKey(), false); } else { arm(c, e.getKey(), Math.max(at, now + 2000)); }"));
        assert!(RAY_NOTIFY_RECEIVER_JAVA.contains("RayAlarm.rearm(c);") && RAY_NOTIFY_JAVA.contains("static void syncBootReceiver(Context c)"));
    }

    /// M442 (IDEAS §121): std/notify en el shell — el manifest declara el permiso y el receiver
    /// (deshabilitado hasta que haya programadas), `exact_alarms` añade SCHEDULE_EXACT_ALARM y fija
    /// la constante de RayNotify, las pulsaciones entran por onCreate/onNewIntent, el diálogo de
    /// permisos no produce un `lifecycle` fantasma y RayBridge atiende las peticiones.
    #[test]
    fn the_shell_ships_the_notification_bridge_and_exact_alarms_are_opt_in() {
        let plain = android_manifest(false, false, None, false, false);
        assert!(plain.contains("android.permission.POST_NOTIFICATIONS"));
        assert!(plain.contains("android.permission.RECEIVE_BOOT_COMPLETED"));
        assert!(plain.contains("android:name=\".RayNotifyReceiver\"") && plain.contains("android:enabled=\"false\""), "{plain}");
        assert!(plain.contains("android.intent.action.BOOT_COMPLETED"));
        assert!(!plain.contains("SCHEDULE_EXACT_ALARM"));
        assert!(android_manifest(false, false, None, true, false).contains("android.permission.SCHEDULE_EXACT_ALARM"));
        assert!(MAIN_ACTIVITY_JAVA.contains("rayNotificationIntent(getIntent());"));
        assert!(MAIN_ACTIVITY_JAVA.contains("if (raySuppressLifecycle) { return; }"));
        assert!(MAIN_ACTIVITY_JAVA.contains("RayNotify.permissionResult(this, grantResults)"));
        assert!(MAIN_ACTIVITY_JAVA.contains("RayNotify.activityGone();") && RAY_NOTIFY_JAVA.contains("static void activityGone()"));
        assert!(RAY_BRIDGE_JAVA.contains("public static native void requestResult(long id, String value, String err);"));
        assert!(RAY_BRIDGE_JAVA.contains("public static void onRequest(String id, String op, String arg)"));
        assert!(RAY_NOTIFY_JAVA.contains("a.requestPermissions(new String[] { POST }, PERMISSION_REQUEST);"));
        // M447 (pepa #331): el sonido del canal viene de res/raw (`[android] sounds`).
        assert!(RAY_NOTIFY_JAVA.contains("ch.setSound(uri, new android.media.AudioAttributes.Builder()") && RAY_NOTIFY_JAVA.contains("getIdentifier(sound.toLowerCase(), \"raw\""));
        assert!(RAY_NOTIFY_JAVA.contains("setAndAllowWhileIdle(AlarmManager.RTC_WAKEUP"));
        assert!(RAY_NOTIFY_RECEIVER_JAVA.contains("Intent.ACTION_BOOT_COMPLETED.equals(action)"));
        let base = std::env::temp_dir().join(format!("ray_android_notify_{}", std::process::id()));
        for (exact, want) in [(true, "EXACT_ALARMS = true;"), (false, "EXACT_ALARMS = false;")] {
            let _ = std::fs::remove_dir_all(&base);
            write_project(&base, "App", "org.example.app", "1.0.0", "'arm64-v8a'", false, false, false, None, exact, false).unwrap();
            let notify = std::fs::read_to_string(base.join("app/src/main/java/org/raylang/shell/RayNotify.java")).unwrap();
            assert!(notify.contains(want) && !notify.contains("/*RAY_EXACT_ALARMS*/"), "exact={exact}");
            assert!(base.join("app/src/main/java/org/raylang/shell/RayNotifyReceiver.java").is_file());
            let main = std::fs::read_to_string(base.join("app/src/main/java/org/raylang/shell/MainActivity.java")).unwrap();
            assert!(!main.contains("/*RAY_DEV_LINK_NEWINTENT*/"), "sin marcadores");
        }
        let _ = std::fs::remove_dir_all(&base);
    }
}
