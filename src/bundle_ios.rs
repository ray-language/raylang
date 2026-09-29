//! §80b — `ray bundle --ios`: el proyecto Xcode GENERADO de una app iOS. La app es un SHELL
//! delgado en Objective-C (UIWindow + WKWebView a pantalla completa) que linkea el staticlib
//! del programa (`ray build --native --lib`): registra sus handlers con `ray_ui_set_handlers`,
//! llama `ray_start()` y desde ahí el programa raylang manda — su webserver embebido sirve la
//! UI y `ui.open(title, url)` le entrega la URL al webview. MISMO FUENTE que el escritorio.
//!
//! Decisiones de plantilla (revisadas en el plan): xcconfig POR SDK para elegir el `.a`
//! (dispositivo y simulador son AMBOS arm64 — un lipo es imposible; el xcframework queda para
//! v2), pbxproj MÍNIMO con UUIDs sintéticos (24 hex, únicos en el archivo) y objectVersion 56,
//! todo lo afinable en el xcconfig; ciclo de vida `UIScene` (manifest en el Info.plist +
//! SceneDelegate: el camino clásico solo-AppDelegate ya avisa deprecación en consola y Apple
//! anuncia assert futuro) — el webview y `ray_start` viven en la escena porque el orden real
//! es didFinishLaunching → willConnectToSession; firma: el simulador no la necesita (el smoke
//! compila con CODE_SIGNING_ALLOWED=NO); dispositivo = abrir en Xcode y elegir team
//! (documentado).

use std::path::Path;

/// El main del shell: UIApplicationMain clásico con nuestro AppDelegate.
const MAIN_M: &str = r#"#import <UIKit/UIKit.h>
#import "AppDelegate.h"

int main(int argc, char *argv[]) {
    @autoreleasepool {
        return UIApplicationMain(argc, argv, nil, NSStringFromClass([AppDelegate class]));
    }
}
"#;

const APP_DELEGATE_H: &str = r#"#import <UIKit/UIKit.h>

@interface AppDelegate : UIResponder <UIApplicationDelegate>
@property (strong, nonatomic) UIWindow *window;
@end
"#;

/// Con `UIScene`, el AppDelegate queda en el arranque del proceso; la ventana, el webview y
/// `ray_start` viven en el SceneDelegate (el ciclo real es didFinishLaunching → connect).
const APP_DELEGATE_M: &str = r#"#import "AppDelegate.h"
/*RAY_AUDIO_IMPORT*/

@implementation AppDelegate

- (BOOL)application:(UIApplication *)application
    didFinishLaunchingWithOptions:(NSDictionary *)launchOptions {
    /*RAY_AUDIO_SESSION*/
    return YES;
}

@end
"#;

const SCENE_DELEGATE_H: &str = r#"#import <UIKit/UIKit.h>

@interface SceneDelegate : UIResponder <UIWindowSceneDelegate>
@property (strong, nonatomic) UIWindow *window;
@end
"#;

/// El corazón del shell: registra los handlers ANTES de ray_start (contrato del staticlib:
/// strings NUL-terminated válidos solo durante la llamada → se COPIAN antes de despachar al
/// hilo principal, que es donde WebKit exige vivir). ray_start corre UNA vez (dispatch_once):
/// si iOS desconecta y reconecta la escena, el programa sigue vivo — el webview nuevo recarga
/// la última URL entregada por ui.open (rayLastURL), no re-arranca el programa. ray_open
/// también la guarda por si llega ANTES de que la escena conecte (programa madrugador).
const SCENE_DELEGATE_M: &str = r#"#import "SceneDelegate.h"
#import <WebKit/WebKit.h>

extern void ray_ui_set_handlers(void (*open)(const char *, const char *),
                                void (*eval)(const char *));
extern void ray_ui_push_event(const char *kind, long long window, const char *tag);
extern int ray_start(void);
extern long long ray_ui_scheme_open(const char *url, const char *method, const char *range,
                                    const char *if_none_match);
extern int ray_ui_scheme_status(long long h);
extern const char *ray_ui_scheme_headers(long long h);
extern long long ray_ui_scheme_read(long long h, unsigned char *buf, long long cap);
extern void ray_ui_scheme_close(long long h);
extern void ray_ui_shell_capabilities(int caps); // M323: 1 = este shell sirve ray://app

// M322 — `ray://app/…` servido desde el programa (WKURLSchemeHandler; el MISMO resolver que el
// shell de macOS: montajes, Range, ETag/304, MIME). El cuerpo se lee por trozos en una cola
// global y se entrega en el hilo principal; una tarea parada por WebKit queda en `_stopped`
// (consultado SOLO en el hilo principal, donde también llega stop) — entregar a una tarea parada
// lanza una excepción.
@interface RaySchemeHandler : NSObject <WKURLSchemeHandler>
@end

@implementation RaySchemeHandler {
    NSMutableSet *_stopped;
}

- (instancetype)init {
    if ((self = [super init])) {
        _stopped = [NSMutableSet set];
    }
    return self;
}

- (void)webView:(WKWebView *)webView startURLSchemeTask:(id<WKURLSchemeTask>)task {
    NSURLRequest *req = task.request;
    NSString *range = [req valueForHTTPHeaderField:@"Range"];
    NSString *inm = [req valueForHTTPHeaderField:@"If-None-Match"];
    long long h = ray_ui_scheme_open(req.URL.absoluteString.UTF8String,
                                     (req.HTTPMethod ?: @"GET").UTF8String,
                                     range.UTF8String, inm.UTF8String);
    if (h == 0) {
        [task didFailWithError:[NSError errorWithDomain:NSURLErrorDomain
                                                   code:NSURLErrorUnsupportedURL
                                               userInfo:nil]];
        return;
    }
    NSMutableDictionary *headers = [NSMutableDictionary dictionary];
    const char *raw = ray_ui_scheme_headers(h);
    NSString *text = raw ? [NSString stringWithUTF8String:raw] : @"";
    for (NSString *line in [text componentsSeparatedByString:@"\n"]) {
        NSRange sep = [line rangeOfString:@": "];
        if (sep.location != NSNotFound) {
            headers[[line substringToIndex:sep.location]] = [line substringFromIndex:sep.location + 2];
        }
    }
    NSHTTPURLResponse *resp = [[NSHTTPURLResponse alloc] initWithURL:req.URL
                                                          statusCode:ray_ui_scheme_status(h)
                                                         HTTPVersion:@"HTTP/1.1"
                                                        headerFields:headers];
    [task didReceiveResponse:resp];
    NSValue *key = [NSValue valueWithPointer:(__bridge void *)task];
    dispatch_async(dispatch_get_global_queue(QOS_CLASS_USER_INITIATED, 0), ^{
      const long long cap = 256 * 1024;
      unsigned char *buf = malloc(cap);
      __block BOOL stopped = NO;
      for (;;) {
          long long n = ray_ui_scheme_read(h, buf, cap);
          if (n <= 0) {
              break;
          }
          NSData *data = [NSData dataWithBytes:buf length:(NSUInteger)n];
          dispatch_sync(dispatch_get_main_queue(), ^{
            stopped = [self->_stopped containsObject:key];
            if (!stopped) {
                [task didReceiveData:data];
            }
          });
          if (stopped) {
              break;
          }
      }
      free(buf);
      ray_ui_scheme_close(h);
      dispatch_async(dispatch_get_main_queue(), ^{
        if (![self->_stopped containsObject:key]) {
            [task didFinish];
        }
        [self->_stopped removeObject:key];
      });
    });
}

- (void)webView:(WKWebView *)webView stopURLSchemeTask:(id<WKURLSchemeTask>)task {
    [_stopped addObject:[NSValue valueWithPointer:(__bridge void *)task]];
}

@end

// M152 — el puente IPC: window.ray.send(text) llega aquí y se empuja como evento "message"
// (window 0: el shell no conoce el handle del programa; documentado). Clase DEDICADA — el
// SceneDelegate como handler crearía un ciclo de retención
// window -> ... -> userContentController -> (strong) delegate -> window.
@interface RayMsgHandler : NSObject <WKScriptMessageHandler>
@end

@implementation RayMsgHandler
- (void)userContentController:(WKUserContentController *)controller
      didReceiveScriptMessage:(WKScriptMessage *)message {
    if (!message.frameInfo.isMainFrame) {
        return; // M159: solo el main frame habla con el programa (paridad con macOS)
    }
    if (![message.body isKindOfClass:[NSString class]]) {
        return; // solo strings v1 (paridad con escritorio)
    }
    ray_ui_push_event("message", 0, [(NSString *)message.body UTF8String]);
}
@end

// El MISMO shim que inyecta el escritorio (ray_runtime::ui::RAY_JS_SHIM, M152/M157:
// send + request/Promise + _deliver).
static NSString *const rayJsShim =
    @"(function(){var p={},n=0;function e(t){return typeof t===\"string\"?t:JSON.stringify(t)}"
    @"function q(s){window.webkit.messageHandlers.ray.postMessage(String(s).replace(/\\u0000/g,\"\"))}"
    @"window.ray={send:function(t){q(e(t))},request:function(t){n=n+1;var i=n;"
    @"return new Promise(function(r){p[i]=r;q(\"\\u0001q\\u0001\"+i+\"\\u0001\"+e(t))})},"
    @"_deliver:function(i,v){var r=p[i];if(r){delete p[i];r(v)}},"
    @"_deliver_json:function(i,t){var r=p[i];if(r){delete p[i];r(JSON.parse(t))}}}})();";

static WKWebView *rayWebView = nil;
static NSString *rayLastURL = nil;

static void ray_open(const char *title, const char *url) {
    NSString *u = [NSString stringWithUTF8String:url]; // copiar ANTES de despachar
    dispatch_async(dispatch_get_main_queue(), ^{
      rayLastURL = u;
      [rayWebView loadRequest:[NSURLRequest requestWithURL:[NSURL URLWithString:u]]];
    });
}

static void ray_eval(const char *js) {
    NSString *s = [NSString stringWithUTF8String:js];
    dispatch_async(dispatch_get_main_queue(), ^{
      [rayWebView evaluateJavaScript:s completionHandler:nil];
    });
}

@implementation SceneDelegate

- (void)scene:(UIScene *)scene
    willConnectToSession:(UISceneSession *)session
                 options:(UISceneConnectionOptions *)connectionOptions {
    UIWindowScene *windowScene = (UIWindowScene *)scene;
    self.window = [[UIWindow alloc] initWithWindowScene:windowScene];
    UIViewController *vc = [UIViewController new];
    // M152: el puente se (re)instala EN CADA conexión de escena — el webview muere y renace
    // con ella (sceneDidDisconnect lo anula), así que esto va fuera del dispatch_once.
    WKWebViewConfiguration *cfg = [[WKWebViewConfiguration alloc] init];
    [cfg setURLSchemeHandler:[RaySchemeHandler new] forURLScheme:@"ray"]; // M322: ray://app/…
    [cfg.userContentController addScriptMessageHandler:[RayMsgHandler new] name:@"ray"];
    [cfg.userContentController
        addUserScript:[[WKUserScript alloc] initWithSource:rayJsShim
                                             injectionTime:WKUserScriptInjectionTimeAtDocumentStart
                                          forMainFrameOnly:YES]];
    rayWebView = [[WKWebView alloc] initWithFrame:vc.view.bounds configuration:cfg];
    /*RAY_DEVTOOLS*/
    rayWebView.autoresizingMask =
        UIViewAutoresizingFlexibleWidth | UIViewAutoresizingFlexibleHeight;
    [vc.view addSubview:rayWebView];
    self.window.rootViewController = vc;
    [self.window makeKeyAndVisible];
    if (rayLastURL != nil) { // reconexión: el programa ya entregó su URL
        [rayWebView loadRequest:[NSURLRequest requestWithURL:[NSURL URLWithString:rayLastURL]]];
    }
    static dispatch_once_t rayOnce;
    dispatch_once(&rayOnce, ^{
      ray_ui_set_handlers(ray_open, ray_eval);
      ray_ui_shell_capabilities(1);
      ray_start();
    });
}

- (void)sceneDidDisconnect:(UIScene *)scene {
    rayWebView = nil; // la vista muere con la escena; el programa sigue
}

- (void)sceneDidEnterBackground:(UIScene *)scene {
    ray_ui_push_event("lifecycle", 0, "background");
}

- (void)sceneWillEnterForeground:(UIScene *)scene {
    ray_ui_push_event("lifecycle", 0, "foreground");
}

@end
"#;

/// Info.plist del shell (placeholders de build settings; `UILaunchScreen` vacío = pantalla de
/// lanzamiento por defecto sin storyboard; ATS con local networking como en el .app de mac).
/// M309 (findings #45): la versión mínima de iOS del proyecto generado (`App.xcconfig`) — y la que
/// `ray build --native --target *-apple-ios*` exporta como `IPHONEOS_DEPLOYMENT_TARGET` para que
/// los objetos C de las dependencias (ring) declaren el mismo `minos`.
pub const IOS_DEPLOYMENT_TARGET: &str = "15.0";

const INFO_PLIST: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleDevelopmentRegion</key><string>en</string>
  <key>CFBundleExecutable</key><string>$(EXECUTABLE_NAME)</string>
  <key>CFBundleIdentifier</key><string>$(PRODUCT_BUNDLE_IDENTIFIER)</string>
  <key>CFBundleInfoDictionaryVersion</key><string>6.0</string>
  <key>CFBundleName</key><string>$(PRODUCT_NAME)</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleShortVersionString</key><string>$(MARKETING_VERSION)</string>
  <key>CFBundleVersion</key><string>1</string>
  <key>UILaunchScreen</key><dict/>
  <key>UIApplicationSceneManifest</key>
  <dict>
    <key>UIApplicationSupportsMultipleScenes</key><false/>
    <key>UISceneConfigurations</key>
    <dict>
      <key>UIWindowSceneSessionRoleApplication</key>
      <array>
        <dict>
          <key>UISceneConfigurationName</key><string>Default</string>
          <key>UISceneDelegateClassName</key><string>SceneDelegate</string>
        </dict>
      </array>
    </dict>
  </dict>
  <key>NSAppTransportSecurity</key>
  <dict><key>NSAllowsLocalNetworking</key><true/></dict>
</dict>
</plist>
"#;

/// M309 (findings #51): el Info.plist del shell con las claves extra (`[app.plist]` del ray.toml y
/// `NSLocalNetworkUsageDescription` cuando toca), insertadas antes del cierre del diccionario.
pub fn info_plist(extra: &[(String, crate::manifest::PlistValue)]) -> String {
    if extra.is_empty() {
        return INFO_PLIST.to_string();
    }
    let esc = |t: &str| t.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;");
    let mut keys = String::new();
    for (k, v) in extra {
        let value = match v {
            crate::manifest::PlistValue::Str(s) => format!("<string>{}</string>", esc(s)),
            crate::manifest::PlistValue::Bool(true) => "<true/>".to_string(),
            crate::manifest::PlistValue::Bool(false) => "<false/>".to_string(),
            crate::manifest::PlistValue::Array(xs) => {
                let items: String = xs.iter().map(|x| format!("<string>{}</string>", esc(x))).collect();
                format!("<array>{items}</array>")
            }
        };
        keys.push_str(&format!("  <key>{}</key>{value}\n", esc(k)));
    }
    INFO_PLIST.replacen("</dict>\n</plist>", &format!("{keys}</dict>\n</plist>"), 1)
}

/// M151 (raydesk #9): la firma que el xcconfig generado debe llevar. `team` viene de
/// `[ios] development_team` en ray.toml o, en su defecto, PRESERVADA del `App.xcconfig`
/// anterior (Xcode la escribe ahí al elegir el equipo; regenerar la borraba en cada bundle).
#[derive(Default, Clone)]
pub struct Signing {
    pub team: Option<String>,
    pub style: Option<String>,
}

impl Signing {
    /// Resuelve la firma final: el manifest manda; lo preservado rellena; con team y sin
    /// estilo, `Automatic` (lo que Xcode escribe al elegir equipo en Signing & Teams).
    pub fn resolve(manifest_team: Option<&str>, previous: &Signing) -> Signing {
        let team = manifest_team.map(str::to_string).or_else(|| previous.team.clone());
        let style = previous.style.clone().or_else(|| team.as_ref().map(|_| "Automatic".to_string()));
        Signing { team, style }
    }

    /// Extrae `DEVELOPMENT_TEAM`/`CODE_SIGN_STYLE` de un `App.xcconfig` existente (primer match
    /// de cada clave; formato `CLAVE = valor` del propio generador y de Xcode).
    pub fn from_xcconfig(text: &str) -> Signing {
        let grab = |key: &str| {
            text.lines()
                .filter_map(|l| l.split_once('='))
                .find(|(k, _)| k.trim() == key)
                .map(|(_, v)| v.trim().to_string())
                .filter(|v| !v.is_empty())
        };
        Signing { team: grab("DEVELOPMENT_TEAM"), style: grab("CODE_SIGN_STYLE") }
    }
}

/// Las opciones del shell generado (M324): icono en el catálogo (ray808 #15) y audio en
/// segundo plano (ray808 #18); `devtools` = M231.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct ShellOptions {
    pub devtools: bool,
    pub icon: bool,
    pub background_audio: bool,
}

/// El xcconfig: TODO lo afinable vive aquí (el pbxproj solo lo referencia). La elección del
/// `.a` por SDK es la pieza clave: dispositivo y simulador son ambos arm64.
fn xcconfig(name: &str, bundle_id: &str, version: &str, signing: &Signing, opts: ShellOptions) -> String {
    let mut sign = String::new();
    if let Some(style) = &signing.style {
        sign.push_str(&format!("CODE_SIGN_STYLE = {style}\n"));
    }
    if let Some(team) = &signing.team {
        sign.push_str(&format!("DEVELOPMENT_TEAM = {team}\n"));
    }
    // M324 (ray808 #15): el icono del catálogo lo compila Xcode solo si el ajuste lo nombra.
    if opts.icon {
        sign.push_str("ASSETCATALOG_COMPILER_APPICON_NAME = AppIcon\n");
    }
    // M324 (ray808 #18): std/audio en iOS es AudioQueue (AudioToolbox); la sesión de fondo, AVFoundation.
    let avf = if opts.background_audio { " -framework AVFoundation" } else { "" };
    format!(
        "// Generado por `ray bundle --ios` — ajusta aquí, no en el pbxproj.\n\
         PRODUCT_NAME = {name}\n\
         PRODUCT_BUNDLE_IDENTIFIER = {bundle_id}\n\
         MARKETING_VERSION = {version}\n\
         IPHONEOS_DEPLOYMENT_TARGET = 15.0\n\
         // Solo arm64: dispositivo y simulador moderno (el .a no trae x86_64; un Mac Intel\n\
         // exigiría además el target x86_64-apple-ios — fuera del v1).\n\
         ARCHS = arm64\n\
         INFOPLIST_FILE = Shell/Info.plist\n\
         TARGETED_DEVICE_FAMILY = 1,2\n\
         SDKROOT = iphoneos\n\
         CLANG_ENABLE_MODULES = YES\n\
         CLANG_ENABLE_OBJC_ARC = YES\n\
         // El staticlib del programa raylang: mismo nombre en ambos dirs, la RUTA elige por SDK\n\
         // (dispositivo y simulador son ambos arm64 — jamás un lipo).\n\
         LIBRARY_SEARCH_PATHS[sdk=iphoneos*] = $(PROJECT_DIR)/libs\n\
         LIBRARY_SEARCH_PATHS[sdk=iphonesimulator*] = $(PROJECT_DIR)/libs-sim\n\
         OTHER_LDFLAGS = -lray_app -framework WebKit -framework AudioToolbox{avf} -lobjc\n\
         {sign}"
    )
}

/// El pbxproj sintético: UN target de app, tres fases (Sources/Frameworks/Resources), configs
/// Debug y Release colgando del xcconfig. UUIDs de 24 hex FIJOS (únicos dentro del archivo — es
/// todo lo que Xcode exige); objectVersion 56 (aceptado por Xcode 14+). M324 (ray808 #15): con
/// icono, `Assets.xcassets` entra en el grupo Shell y en la fase Resources — antes el catálogo se
/// escribía pero nada lo compilaba y la app salía con el icono genérico.
fn pbxproj(name: &str, icon: bool) -> String {
    let (res_file, res_build, res_child, res_files) = if icon {
        (
            "\t\t0000000000000000000000F9 /* Assets.xcassets */ = {isa = PBXFileReference; lastKnownFileType = folder.assetcatalog; path = Assets.xcassets; sourceTree = \"<group>\"; };\n",
            "\t\t0000000000000000000000B4 /* Assets.xcassets in Resources */ = {isa = PBXBuildFile; fileRef = 0000000000000000000000F9 /* Assets.xcassets */; };\n",
            ", 0000000000000000000000F9",
            "0000000000000000000000B4",
        )
    } else {
        ("", "", "", "")
    };
    format!(
        r##"// !$*UTF8*$!
{{
	archiveVersion = 1;
	classes = {{
	}};
	objectVersion = 56;
	objects = {{
		0000000000000000000000B1 /* main.m in Sources */ = {{isa = PBXBuildFile; fileRef = 0000000000000000000000F1 /* main.m */; }};
		0000000000000000000000B2 /* AppDelegate.m in Sources */ = {{isa = PBXBuildFile; fileRef = 0000000000000000000000F3 /* AppDelegate.m */; }};
		0000000000000000000000B3 /* SceneDelegate.m in Sources */ = {{isa = PBXBuildFile; fileRef = 0000000000000000000000F8 /* SceneDelegate.m */; }};
{res_build}{res_file}		0000000000000000000000F1 /* main.m */ = {{isa = PBXFileReference; lastKnownFileType = sourcecode.c.objc; path = main.m; sourceTree = "<group>"; }};
		0000000000000000000000F2 /* AppDelegate.h */ = {{isa = PBXFileReference; lastKnownFileType = sourcecode.c.h; path = AppDelegate.h; sourceTree = "<group>"; }};
		0000000000000000000000F3 /* AppDelegate.m */ = {{isa = PBXFileReference; lastKnownFileType = sourcecode.c.objc; path = AppDelegate.m; sourceTree = "<group>"; }};
		0000000000000000000000F4 /* Info.plist */ = {{isa = PBXFileReference; lastKnownFileType = text.plist.xml; path = Info.plist; sourceTree = "<group>"; }};
		0000000000000000000000F7 /* SceneDelegate.h */ = {{isa = PBXFileReference; lastKnownFileType = sourcecode.c.h; path = SceneDelegate.h; sourceTree = "<group>"; }};
		0000000000000000000000F8 /* SceneDelegate.m */ = {{isa = PBXFileReference; lastKnownFileType = sourcecode.c.objc; path = SceneDelegate.m; sourceTree = "<group>"; }};
		0000000000000000000000F5 /* App.xcconfig */ = {{isa = PBXFileReference; lastKnownFileType = text.xcconfig; path = App.xcconfig; sourceTree = "<group>"; }};
		0000000000000000000000F6 /* {name}.app */ = {{isa = PBXFileReference; explicitFileType = wrapper.application; includeInIndex = 0; path = "{name}.app"; sourceTree = BUILT_PRODUCTS_DIR; }};
		0000000000000000000000E1 /* Frameworks */ = {{isa = PBXFrameworksBuildPhase; buildActionMask = 2147483647; files = (); runOnlyForDeploymentPostprocessing = 0; }};
		0000000000000000000000E2 /* Sources */ = {{isa = PBXSourcesBuildPhase; buildActionMask = 2147483647; files = (0000000000000000000000B1, 0000000000000000000000B2, 0000000000000000000000B3); runOnlyForDeploymentPostprocessing = 0; }};
		0000000000000000000000E3 /* Resources */ = {{isa = PBXResourcesBuildPhase; buildActionMask = 2147483647; files = ({res_files}); runOnlyForDeploymentPostprocessing = 0; }};
		0000000000000000000000A1 /* Shell */ = {{isa = PBXGroup; children = (0000000000000000000000F1, 0000000000000000000000F2, 0000000000000000000000F3, 0000000000000000000000F7, 0000000000000000000000F8, 0000000000000000000000F4{res_child}); path = Shell; sourceTree = "<group>"; }};
		0000000000000000000000A2 /* Products */ = {{isa = PBXGroup; children = (0000000000000000000000F6); name = Products; sourceTree = "<group>"; }};
		0000000000000000000000A3 = {{isa = PBXGroup; children = (0000000000000000000000A1, 0000000000000000000000F5, 0000000000000000000000A2); sourceTree = "<group>"; }};
		0000000000000000000000D1 /* {name} */ = {{isa = PBXNativeTarget; buildConfigurationList = 0000000000000000000000C3; buildPhases = (0000000000000000000000E2, 0000000000000000000000E1, 0000000000000000000000E3); buildRules = (); dependencies = (); name = "{name}"; productName = "{name}"; productReference = 0000000000000000000000F6; productType = "com.apple.product-type.application"; }};
		0000000000000000000000D2 /* Project */ = {{isa = PBXProject; attributes = {{ LastUpgradeCheck = 1500; }}; buildConfigurationList = 0000000000000000000000C4; compatibilityVersion = "Xcode 14.0"; developmentRegion = en; hasScannedForEncodings = 0; knownRegions = (en, Base); mainGroup = 0000000000000000000000A3; productRefGroup = 0000000000000000000000A2; projectDirPath = ""; projectRoot = ""; targets = (0000000000000000000000D1); }};
		0000000000000000000000C1 /* Debug */ = {{isa = XCBuildConfiguration; baseConfigurationReference = 0000000000000000000000F5; buildSettings = {{ ONLY_ACTIVE_ARCH = YES; DEBUG_INFORMATION_FORMAT = dwarf; }}; name = Debug; }};
		0000000000000000000000C2 /* Release */ = {{isa = XCBuildConfiguration; baseConfigurationReference = 0000000000000000000000F5; buildSettings = {{ }}; name = Release; }};
		0000000000000000000000C5 /* Debug */ = {{isa = XCBuildConfiguration; baseConfigurationReference = 0000000000000000000000F5; buildSettings = {{ PRODUCT_NAME = "$(TARGET_NAME)"; }}; name = Debug; }};
		0000000000000000000000C6 /* Release */ = {{isa = XCBuildConfiguration; baseConfigurationReference = 0000000000000000000000F5; buildSettings = {{ PRODUCT_NAME = "$(TARGET_NAME)"; }}; name = Release; }};
		0000000000000000000000C3 /* target configs */ = {{isa = XCConfigurationList; buildConfigurations = (0000000000000000000000C5, 0000000000000000000000C6); defaultConfigurationIsVisible = 0; defaultConfigurationName = Release; }};
		0000000000000000000000C4 /* project configs */ = {{isa = XCConfigurationList; buildConfigurations = (0000000000000000000000C1, 0000000000000000000000C2); defaultConfigurationIsVisible = 0; defaultConfigurationName = Release; }};
	}};
	rootObject = 0000000000000000000000D2 /* Project */;
}}
"##
    )
}

/// M316 (findings #91): el README lleva el NOMBRE de la app (antes decía literalmente `<Name>`).
fn readme(name: &str) -> String {
    format!(r#"# iOS app generated by `ray bundle --ios`

- `libs/` and `libs-sim/` hold the program's static library (device / simulator); the xcconfig
  picks one by SDK. **After changing the program or the frontend, rebuilding the library is
  enough**, without touching the Xcode project (or the signing):
  `ray build --native --lib --release --target aarch64-apple-ios -o {name}-ios/libs/libray_app.a`
  (and `--target aarch64-apple-ios-sim -o {name}-ios/libs-sim/libray_app.a` for the simulator).
  Run `ray bundle --ios` again only when `[app]` changes or raylang is updated — with
  `--ios-target device|sim` it builds one side only (the other `.a` is kept if it existed).
  Regenerating rewrites `project.pbxproj` but keeps `{name}.xcodeproj/xcshareddata` (shared
  schemes and their environment variables) and `xcuserdata`.
- Simulator (no signing):
  `xcodebuild -project {name}.xcodeproj -target {name} -sdk iphonesimulator -configuration Debug build CODE_SIGNING_ALLOWED=NO`
  then `xcrun simctl boot <device>` + `install` + `launch`.
- Device (signing): declare `[ios] development_team = "ABCDE12345"` in `ray.toml`, or add
  `DEVELOPMENT_TEAM = ABCDE12345` to `App.xcconfig`. Picking the team in Xcode (Signing &
  Capabilities) writes it to `project.pbxproj`; the next `ray bundle --ios` rescues it from there
  into the xcconfig, but the source of truth is the xcconfig or `ray.toml`.
- The raylang program runs INSIDE the app (staticlib) and `ui.open(title, url)` loads the URL
  in the webview: either `ray://app/…` served from the process (`ui.mount_embed`/`mount_dir`/
  `mount_bytes`, no port, same as the desktop shells) or `http://127.0.0.1:<port>` from its
  embedded webserver. Lifecycle events arrive through `ui.next_event()` as kind="lifecycle",
  tag="background"/"foreground".
- `[app] icon` in `ray.toml` becomes `Shell/Assets.xcassets/AppIcon.appiconset`, wired into the
  Resources phase and `ASSETCATALOG_COMPILER_APPICON_NAME` (regenerate after changing it).
- `std/audio` plays through AudioQueue (AudioToolbox) on iOS too. With `[ios] background_audio =
  true` the shell activates an `AVAudioSession` of category `playback` at launch and the
  Info.plist declares `UIBackgroundModes = ["audio"]`: the program keeps running and sounding
  when the app goes to the background (and with the silent switch on). Without it, iOS suspends
  the process a few seconds after `lifecycle`/`background`.
"#)
}

/// Genera el árbol del proyecto en `dir` (ya creado). Los `.a` los copia el llamador; la
/// `signing` resuelta (manifest > preservada) va al xcconfig.
#[cfg(test)]
mod devtools_tests {
    /// M231: `--devtools` deja `inspectable` en el shell; sin el flag la línea no existe.
    #[test]
    fn devtools_flag_toggles_inspectable_in_the_shell() {
        let base = std::env::temp_dir().join(format!("ray_ios_devtools_{}", std::process::id()));
        for (devtools, want) in [(true, true), (false, false)] {
            let _ = std::fs::remove_dir_all(&base);
            let opts = super::ShellOptions { devtools, ..Default::default() };
            super::write_project(&base, "App", "org.example.app", "1.0.0", &super::Signing::default(), opts, &[]).unwrap();
            let src = std::fs::read_to_string(base.join("Shell/SceneDelegate.m")).unwrap();
            assert_eq!(src.contains("rayWebView.inspectable = YES"), want, "devtools={devtools}");
            assert!(!src.contains("/*RAY_DEVTOOLS*/"), "el marcador no queda en el proyecto");
        }
        let _ = std::fs::remove_dir_all(&base);
    }
}

pub fn write_project(
    dir: &Path,
    name: &str,
    bundle_id: &str,
    version: &str,
    signing: &Signing,
    opts: ShellOptions,
    plist_extra: &[(String, crate::manifest::PlistValue)],
) -> Result<(), String> {
    let ShellOptions { devtools, icon, background_audio } = opts;
    let write = |rel: &str, content: &str| -> Result<(), String> {
        let p = dir.join(rel);
        if let Some(parent) = p.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        std::fs::write(&p, content).map_err(|e| format!("{}: {e}", p.display()))
    };
    write("Shell/main.m", MAIN_M)?;
    write("Shell/AppDelegate.h", APP_DELEGATE_H)?;
    // M324 (ray808 #18): con `[ios] background_audio`, la sesión de audio `playback` se activa al
    // arrancar — sin ella iOS corta el audio al pasar a segundo plano (categoría ambient por
    // defecto) y con el interruptor de silencio. El modo de fondo va en el Info.plist (el llamador
    // añade `UIBackgroundModes`).
    let (audio_import, audio_session) = if background_audio {
        (
            "#import <AVFoundation/AVFoundation.h>",
            "AVAudioSession *session = [AVAudioSession sharedInstance]; // [ios] background_audio\n    \
             [session setCategory:AVAudioSessionCategoryPlayback error:nil];\n    \
             [session setActive:YES error:nil];",
        )
    } else {
        ("", "")
    };
    write(
        "Shell/AppDelegate.m",
        &APP_DELEGATE_M.replace("/*RAY_AUDIO_IMPORT*/\n", &format!("{audio_import}\n")).replace("/*RAY_AUDIO_SESSION*/", audio_session),
    )?;
    write("Shell/SceneDelegate.h", SCENE_DELEGATE_H)?;
    // M231: `--devtools` → `inspectable` (iOS 16.4+): el Web Inspector de Safari (menú Develop del
    // Mac) inspecciona la app en el dispositivo o el simulador. Nunca en un build sin el flag.
    let devtools_line = if devtools {
        "if (@available(iOS 16.4, *)) { rayWebView.inspectable = YES; } // ray bundle --devtools"
    } else {
        ""
    };
    write("Shell/SceneDelegate.m", &SCENE_DELEGATE_M.replace("/*RAY_DEVTOOLS*/", devtools_line))?;
    write("Shell/Info.plist", &info_plist(plist_extra))?;
    write("App.xcconfig", &xcconfig(name, bundle_id, version, signing, opts))?;
    write(&format!("{name}.xcodeproj/project.pbxproj"), &pbxproj(name, icon))?;
    write("README.md", &readme(name))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// M309 (findings #51): las claves extra (`[app.plist]`, NSLocalNetworkUsageDescription) entran
    /// en el Info.plist del shell antes del cierre del diccionario; sin extras, el plist de siempre.
    #[test]
    fn info_plist_takes_extra_keys() {
        assert_eq!(info_plist(&[]), INFO_PLIST);
        let p = info_plist(&[
            ("NSLocalNetworkUsageDescription".to_string(), crate::manifest::PlistValue::Str("X <talks> & more".to_string())),
            ("UIFileSharingEnabled".to_string(), crate::manifest::PlistValue::Bool(true)),
        ]);
        assert!(p.contains("<key>NSLocalNetworkUsageDescription</key><string>X &lt;talks&gt; &amp; more</string>\n  <key>UIFileSharingEnabled</key><true/>\n</dict>\n</plist>"), "{p}");
        assert!(p.contains("NSAllowsLocalNetworking"), "ATS sigue: {p}");
    }

    #[test]
    fn signing_resolution_prefers_the_manifest_and_preserves_the_previous() {
        // M151 (raydesk #9): manifest manda; sin manifest, lo preservado; con team y sin
        // estilo, Automatic (lo que Xcode escribe al elegir equipo).
        let previous = Signing::from_xcconfig(
            "PRODUCT_NAME = X\nCODE_SIGN_STYLE = Automatic\nDEVELOPMENT_TEAM = OLDTEAM123\n",
        );
        assert_eq!(previous.team.as_deref(), Some("OLDTEAM123"));
        let from_manifest = Signing::resolve(Some("NEWTEAM456"), &previous);
        assert_eq!(from_manifest.team.as_deref(), Some("NEWTEAM456"));
        let preserved = Signing::resolve(None, &previous);
        assert_eq!(preserved.team.as_deref(), Some("OLDTEAM123"));
        assert_eq!(preserved.style.as_deref(), Some("Automatic"));
        let no_previous = Signing::resolve(Some("T1"), &Signing::default());
        assert_eq!(no_previous.style.as_deref(), Some("Automatic"));
        let empty = Signing::resolve(None, &Signing::default());
        assert!(empty.team.is_none() && empty.style.is_none());
    }

    #[test]
    fn the_message_handler_only_listens_to_the_main_frame() {
        // M159: la guarda de frames — un iframe no alcanza el puente ni a mano.
        assert!(SCENE_DELEGATE_M.contains("if (!message.frameInfo.isMainFrame)"), "isMainFrame guard");
        // Y la guarda va ANTES de leer el body.
        let guard = SCENE_DELEGATE_M.find("isMainFrame").unwrap();
        let body = SCENE_DELEGATE_M.find("message.body").unwrap();
        assert!(guard < body, "the frame guard precedes the body read");
    }

    /// M322: el shell registra el WKURLSchemeHandler de `ray` ANTES de crear el webview y
    /// resuelve contra la ABI C del runtime (open/status/headers/read/close).
    #[test]
    fn the_shell_serves_the_ray_scheme_from_the_program() {
        assert!(SCENE_DELEGATE_M.contains("setURLSchemeHandler:[RaySchemeHandler new] forURLScheme:@\"ray\"]"));
        let register = SCENE_DELEGATE_M.find("setURLSchemeHandler:").unwrap();
        let create = SCENE_DELEGATE_M.find("[[WKWebView alloc] initWithFrame:").unwrap();
        assert!(register < create, "the scheme handler is registered before the webview exists");
        for sym in ["ray_ui_scheme_open", "ray_ui_scheme_status", "ray_ui_scheme_headers", "ray_ui_scheme_read", "ray_ui_scheme_close"] {
            assert!(SCENE_DELEGATE_M.contains(sym), "{sym} in the shell");
        }
        // Una tarea parada por WebKit no recibe más datos ni didFinish.
        assert!(SCENE_DELEGATE_M.contains("stopURLSchemeTask:"));
        assert!(SCENE_DELEGATE_M.contains("ray_ui_shell_capabilities(1);\n      ray_start();"), "capabilities before ray_start");
        assert!(SCENE_DELEGATE_M.contains("if (![self->_stopped containsObject:key]) {\n            [task didFinish];"));
    }

    /// M324 (ray808 #15): con icono, el catálogo entra en el pbxproj (grupo + fase Resources) y el
    /// xcconfig nombra el AppIcon; sin icono, ni rastro (un catálogo referenciado y ausente rompe el build).
    #[test]
    fn the_icon_is_wired_into_the_project_only_when_present() {
        let with = pbxproj("Demo", true);
        assert!(with.contains("PBXResourcesBuildPhase; buildActionMask = 2147483647; files = (0000000000000000000000B4)"), "{with}");
        assert!(with.contains("path = Assets.xcassets; sourceTree"), "{with}");
        assert!(with.contains("0000000000000000000000F4, 0000000000000000000000F9); path = Shell"), "{with}");
        assert!(with.contains("buildPhases = (0000000000000000000000E2, 0000000000000000000000E1, 0000000000000000000000E3)"), "{with}");
        let without = pbxproj("Demo", false);
        assert!(!without.contains("Assets.xcassets"), "{without}");
        assert!(without.contains("PBXResourcesBuildPhase; buildActionMask = 2147483647; files = ()"), "{without}");
        let opts = ShellOptions { icon: true, ..Default::default() };
        assert!(xcconfig("Demo", "org.raylang.demo", "1.0.0", &Signing::default(), opts).contains("ASSETCATALOG_COMPILER_APPICON_NAME = AppIcon\n"));
        assert!(!xcconfig("Demo", "org.raylang.demo", "1.0.0", &Signing::default(), ShellOptions::default()).contains("ASSETCATALOG"));
    }

    /// M324 (ray808 #18): `[ios] background_audio` activa la sesión `playback` en el AppDelegate y
    /// enlaza AVFoundation; AudioToolbox (std/audio) se enlaza siempre. El plist admite arrays.
    #[test]
    fn background_audio_configures_the_session_and_the_frameworks() {
        let base = std::env::temp_dir().join(format!("ray_ios_audio_{}", std::process::id()));
        for (bg, want) in [(true, true), (false, false)] {
            let _ = std::fs::remove_dir_all(&base);
            let opts = ShellOptions { background_audio: bg, ..Default::default() };
            write_project(&base, "App", "org.example.app", "1.0.0", &Signing::default(), opts, &[]).unwrap();
            let src = std::fs::read_to_string(base.join("Shell/AppDelegate.m")).unwrap();
            assert_eq!(src.contains("[session setCategory:AVAudioSessionCategoryPlayback error:nil];"), want, "bg={bg}");
            assert_eq!(src.contains("#import <AVFoundation/AVFoundation.h>"), want, "bg={bg}");
            assert!(!src.contains("/*RAY_AUDIO"), "sin marcadores: {src}");
            let cfg = std::fs::read_to_string(base.join("App.xcconfig")).unwrap();
            assert_eq!(cfg.contains("-framework AVFoundation"), want, "bg={bg}");
            assert!(cfg.contains("-framework AudioToolbox"), "std/audio siempre enlazable");
        }
        let _ = std::fs::remove_dir_all(&base);
        let p = info_plist(&[("UIBackgroundModes".to_string(), crate::manifest::PlistValue::Array(vec!["audio".to_string()]))]);
        assert!(p.contains("<key>UIBackgroundModes</key><array><string>audio</string></array>"), "{p}");
    }

    #[test]
    fn the_xcconfig_carries_the_resolved_signing() {
        let s = Signing { team: Some("ABC123".into()), style: Some("Automatic".into()) };
        let text = xcconfig("Demo", "org.raylang.demo", "1.0.0", &s, ShellOptions::default());
        assert!(text.contains("DEVELOPMENT_TEAM = ABC123"), "{text}");
        assert!(text.contains("CODE_SIGN_STYLE = Automatic"), "{text}");
        // Y el round-trip: lo que el bundle escribe, la siguiente regeneración lo preserva.
        let back = Signing::from_xcconfig(&text);
        assert_eq!(back.team.as_deref(), Some("ABC123"));
        let bare = xcconfig("Demo", "org.raylang.demo", "1.0.0", &Signing::default(), ShellOptions::default());
        assert!(!bare.contains("DEVELOPMENT_TEAM"), "{bare}");
    }
}
