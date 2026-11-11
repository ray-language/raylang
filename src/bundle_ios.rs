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
#import <UserNotifications/UserNotifications.h>
/*RAY_AUDIO_IMPORT*/

extern void ray_ui_will_terminate(void); // Apaga el audio con rampa antes de morir
extern void ray_ui_push_event(const char *kind, long long window, const char *tag);

// std/notify: el delegate del centro de notificaciones vive en el shell (debe existir antes de que
// termine didFinishLaunching para recibir la pulsación que ARRANCA la app); el runtime raylang
// crea y programa las notificaciones por su cuenta (UNUserNotificationCenter desde Rust).
@interface AppDelegate () <UNUserNotificationCenterDelegate>
@end

@implementation AppDelegate

- (BOOL)application:(UIApplication *)application
    didFinishLaunchingWithOptions:(NSDictionary *)launchOptions {
    [UNUserNotificationCenter currentNotificationCenter].delegate = self;
    /*RAY_AUDIO_SESSION*/
    return YES;
}

// Con la app delante la notificación se muestra igual (banner + lista + sonido).
- (void)userNotificationCenter:(UNUserNotificationCenter *)center
       willPresentNotification:(UNNotification *)notification
         withCompletionHandler:(void (^)(UNNotificationPresentationOptions))completionHandler {
    completionHandler(UNNotificationPresentationOptionBanner | UNNotificationPresentationOptionList
                      | UNNotificationPresentationOptionSound);
}

// Una pulsación (la notificación o una de sus acciones) → evento ("notification", 0, tag): el id a
// secas, o `id\taction\ttext\tdata` (lo deshace notify.event_tap). Si la app arrancó por ella, el
// evento queda encolado antes de que el programa empiece.
- (void)userNotificationCenter:(UNUserNotificationCenter *)center
    didReceiveNotificationResponse:(UNNotificationResponse *)response
             withCompletionHandler:(void (^)(void))completionHandler {
    NSDictionary *info = response.notification.request.content.userInfo;
    NSString *ident = [info[@"tag"] isKindOfClass:[NSString class]] ? info[@"tag"] : response.notification.request.identifier;
    NSString *data = [info[@"data"] isKindOfClass:[NSString class]] ? info[@"data"] : @"";
    NSString *action = [response.actionIdentifier isEqualToString:UNNotificationDefaultActionIdentifier] ? @"" : response.actionIdentifier;
    NSString *text = [response isKindOfClass:[UNTextInputNotificationResponse class]] ? ((UNTextInputNotificationResponse *)response).userText : @"";
    NSString *tag = (action.length == 0 && text.length == 0 && data.length == 0)
        ? ident
        : [NSString stringWithFormat:@"%@\t%@\t%@\t%@", ident, action, text, data];
    ray_ui_push_event("notification", 0, tag.UTF8String);
    completionHandler();
}

// Cerrar la app desde el selector con el audio sonando mataba el proceso con la
// cola a mitad de onda (clic). Con la sesión de reproducción activa iOS avisa aquí antes.
- (void)applicationWillTerminate:(UIApplication *)application {
    ray_ui_will_terminate();
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
#import <UniformTypeIdentifiers/UniformTypeIdentifiers.h>

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
extern void ray_ui_shell_capabilities(int caps); // 1 = este shell sirve ray://app
// M447 (pepa #334): el `WindowOptions.background` de la ventana que ray_open está recibiendo (NULL = sin fijar).
extern const char *ray_ui_open_background(void);
/*RAY_ALARM_SHELL*/
// M382: el selector de archivos del sistema (`ui.pick_file*`).
extern void ray_ui_set_pick_handler(void (*pick)(long long, const char *, const char *, int,
                                                 const char *));
extern void ray_ui_pick_result(long long id, const char *paths, const char *err);
/*RAY_DEV_EXTERN*/

// `ray://app/…` servido desde el programa (WKURLSchemeHandler; el MISMO resolver que el
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

// El puente IPC: window.ray.send(text) llega aquí y se empuja como evento "message"
// (window 0: el shell no conoce el handle del programa; documentado). Clase DEDICADA — el
// SceneDelegate como handler crearía un ciclo de retención
// window -> ... -> userContentController -> (strong) delegate -> window.
@interface RayMsgHandler : NSObject <WKScriptMessageHandler>
@end

@implementation RayMsgHandler
- (void)userContentController:(WKUserContentController *)controller
      didReceiveScriptMessage:(WKScriptMessage *)message {
    if (!message.frameInfo.isMainFrame) {
        return; // Solo el main frame habla con el programa (paridad con macOS)
    }
    if (![message.body isKindOfClass:[NSString class]]) {
        return; // solo strings v1 (paridad con escritorio)
    }
    ray_ui_push_event("message", 0, [(NSString *)message.body UTF8String]);
}
@end

// El MISMO shim que inyecta el escritorio (ray_runtime::ui::RAY_JS_SHIM:
// send + request/Promise + _deliver).
static NSString *const rayJsShim =
    @"(function(){var p={},n=0;function e(t){return typeof t===\"string\"?t:JSON.stringify(t)}"
    @"function q(s){window.webkit.messageHandlers.ray.postMessage(String(s).replace(/\\u0000/g,\"\"))}"
    @"window.ray={send:function(t){q(e(t))},request:function(t){n=n+1;var i=n;"
    @"return new Promise(function(r){p[i]=r;q(\"\\u0001q\\u0001\"+i+\"\\u0001\"+e(t))})},"
    @"_deliver:function(i,v){var r=p[i];if(r){delete p[i];r(v)}},"
    @"_deliver_json:function(i,t){var r=p[i];if(r){delete p[i];r(JSON.parse(t))}}}})();";

// rayround #182: sin --devtools los errores de la página no se veían en ningún sitio. Los
// errores no capturados, los rechazos de promesas y console.error/warn llegan aquí y salen por
// stderr —la consola de `simctl launch --console-pty` y de Xcode— con prefijo `[page]`.
@interface RayLogHandler : NSObject <WKScriptMessageHandler>
@end

@implementation RayLogHandler
- (void)userContentController:(WKUserContentController *)controller
      didReceiveScriptMessage:(WKScriptMessage *)message {
    if (![message.body isKindOfClass:[NSString class]]) {
        return;
    }
    fprintf(stderr, "%s\n", [(NSString *)message.body UTF8String]);
    fflush(stderr);
}
@end

static NSString *const rayLogShim =
    @"(function(){function l(s){window.webkit.messageHandlers.rayLog.postMessage(String(s))}"
    @"window.addEventListener('error',function(e){l('[page] '+(e.message||String(e.error||e))+' @'+(e.filename||'')+':'+(e.lineno||0)+':'+(e.colno||0))});window.addEventListener('unhandledrejection',function(e){var r=e.reason;l('[page] unhandled rejection: '+(r&&r.message||String(r))+(r&&r.stack?' @'+r.stack.split('\\n')[0]:''))});var ce=console.error,cw=console.warn;console.error=function(){l('[page] console.error: '+[].slice.call(arguments).join(' '));ce.apply(console,arguments)};console.warn=function(){l('[page] console.warn: '+[].slice.call(arguments).join(' '));cw.apply(console,arguments)};"
    @"})();";

static WKWebView *rayWebView = nil;
static NSString *rayLastURL = nil;
static UIColor *rayLastBackground = nil; // M447: el WindowOptions.background del programa (sobrevive a la escena)

// M382: `ui.pick_file*` → UIDocumentPickerViewController en modo COPIA (`asCopy:YES`): el
// sistema deja una copia en el tmp de la app y las rutas que vuelven se leen con std/fs sin
// permisos de alcance. El delegate vive en una estática mientras el picker está en pantalla.
@interface RayPickDelegate : NSObject <UIDocumentPickerDelegate>
@property long long pickId;
@end

static RayPickDelegate *rayPickDelegate = nil;

@implementation RayPickDelegate
- (void)documentPicker:(UIDocumentPickerViewController *)controller
    didPickDocumentsAtURLs:(NSArray<NSURL *> *)urls {
    NSMutableArray<NSString *> *paths = [NSMutableArray new];
    for (NSURL *u in urls) {
        if (u.isFileURL) { [paths addObject:u.path]; }
    }
    ray_ui_pick_result(self.pickId, [[paths componentsJoinedByString:@"\n"] UTF8String], NULL);
    rayPickDelegate = nil;
}
- (void)documentPickerWasCancelled:(UIDocumentPickerViewController *)controller {
    ray_ui_pick_result(self.pickId, "", NULL);
    rayPickDelegate = nil;
}
@end

static void ray_pick(long long id, const char *kind, const char *title, int multiple,
                     const char *exts) {
    NSString *e = [NSString stringWithUTF8String:exts ? exts : ""]; // copiar ANTES de despachar
    dispatch_async(dispatch_get_main_queue(), ^{
      if (rayPickDelegate != nil) {
          ray_ui_pick_result(id, "", "ui: a file picker is already open");
          return;
      }
      UIViewController *root = rayWebView.window.rootViewController;
      if (root == nil) {
          ray_ui_pick_result(id, "", "ui: no window to present the file picker");
          return;
      }
      NSMutableArray<UTType *> *types = [NSMutableArray new];
      for (NSString *ext in [e componentsSeparatedByString:@","]) {
          if (ext.length == 0) { continue; }
          UTType *t = [UTType typeWithFilenameExtension:ext];
          if (t != nil) { [types addObject:t]; }
      }
      if (types.count == 0) { [types addObject:UTTypeItem]; }
      UIDocumentPickerViewController *picker =
          [[UIDocumentPickerViewController alloc] initForOpeningContentTypes:types asCopy:YES];
      picker.allowsMultipleSelection = multiple != 0;
      RayPickDelegate *d = [RayPickDelegate new];
      d.pickId = id;
      rayPickDelegate = d;
      picker.delegate = d;
      [root presentViewController:picker animated:YES completion:nil];
    });
}

// M447 (pepa #334): `#rrggbb` → UIColor (nil si no es un color).
static UIColor *rayColorFromHex(const char *hex) {
    if (hex == NULL || strlen(hex) != 7 || hex[0] != '#') { return nil; }
    unsigned int v = 0;
    if (sscanf(hex + 1, "%06x", &v) != 1) { return nil; }
    return [UIColor colorWithRed:((v >> 16) & 0xff) / 255.0 green:((v >> 8) & 0xff) / 255.0 blue:(v & 0xff) / 255.0 alpha:1.0];
}

static void ray_open(const char *title, const char *url) {
    NSString *u = [NSString stringWithUTF8String:url]; // copiar ANTES de despachar
    UIColor *bg = rayColorFromHex(ray_ui_open_background()); // válido solo durante la llamada
    dispatch_async(dispatch_get_main_queue(), ^{
      rayLastURL = u;
      if (bg != nil) { // pepa #334: lo que se ve hasta que la página pinta (y detrás de ella)
        rayLastBackground = bg;
        rayWebView.window.backgroundColor = bg;
        rayWebView.window.rootViewController.view.backgroundColor = bg;
      }
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
    // M447 (pepa #334): el color de arranque (`[ios] background`, catálogo LaunchBackground) sigue
    // bajo la página hasta que esta pinta — sin él, el blanco del sistema.
    // Prioridad: el fondo que pidió el programa (reconexión), el color de arranque, el del sistema
    // (el webview es transparente: sin esto una app sin color vería negro donde antes había blanco).
    UIColor *launch = rayLastBackground ?: ([UIColor colorNamed:@"LaunchBackground"] ?: UIColor.systemBackgroundColor);
    self.window.backgroundColor = launch;
    vc.view.backgroundColor = launch;
    // El puente se (re)instala EN CADA conexión de escena — el webview muere y renace
    // con ella (sceneDidDisconnect lo anula), así que esto va fuera del dispatch_once.
    WKWebViewConfiguration *cfg = [[WKWebViewConfiguration alloc] init];
    [cfg setURLSchemeHandler:[RaySchemeHandler new] forURLScheme:@"ray"]; // ray://app/…
    [cfg.userContentController addScriptMessageHandler:[RayMsgHandler new] name:@"ray"];
    [cfg.userContentController
        addUserScript:[[WKUserScript alloc] initWithSource:rayJsShim
                                             injectionTime:WKUserScriptInjectionTimeAtDocumentStart
                                          forMainFrameOnly:YES]];
    [cfg.userContentController addScriptMessageHandler:[RayLogHandler new] name:@"rayLog"];
    [cfg.userContentController
        addUserScript:[[WKUserScript alloc] initWithSource:rayLogShim
                                             injectionTime:WKUserScriptInjectionTimeAtDocumentStart
                                          forMainFrameOnly:YES]];
    rayWebView = [[WKWebView alloc] initWithFrame:vc.view.bounds configuration:cfg];
    // pepa #328: UIKit ajustaba los insets del scroll por las zonas seguras y WebKit encogía el
    // viewport (759 pt en vez de 852 con la isla dinámica): la página ya reserva las zonas con
    // env(safe-area-inset-*) y viewport-fit=cover, así que el webview ocupa la pantalla entera.
    rayWebView.scrollView.contentInsetAdjustmentBehavior = UIScrollViewContentInsetAdjustmentNever;
    // pepa #334: el webview es transparente — el fondo lo pone la vista (color de arranque o el
    // `WindowOptions.background` del programa), nunca el blanco por defecto de WebKit.
    rayWebView.opaque = NO;
    rayWebView.backgroundColor = UIColor.clearColor;
    rayWebView.scrollView.backgroundColor = UIColor.clearColor;
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
      // El nombre visible de la app, para la pantalla de emparejamiento de la librería de desarrollo.
      NSString *rayLabel = [[NSBundle mainBundle] objectForInfoDictionaryKey:@"CFBundleDisplayName"]
          ?: [[NSBundle mainBundle] objectForInfoDictionaryKey:@"CFBundleName"];
      if (rayLabel != nil) { setenv("RAY_DEV_APP_NAME", [rayLabel UTF8String], 1); }
      ray_ui_set_handlers(ray_open, ray_eval);
      ray_ui_set_pick_handler(ray_pick); // M382 /*RAY_ALARM_REGISTER*/
      ray_ui_shell_capabilities(1);
      ray_start(); /*RAY_ALARM_STARTED*/
    });
    /*RAY_DEV_LINK_CONNECT*/
}
/*RAY_DEV_LINK_METHOD*/

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

/// M448b (IDEAS §122, pepa #341): `std/alarm` en iOS 26+ sobre **AlarmKit**, que solo existe en
/// Swift — la primera pieza Swift del shell, y solo entra al proyecto cuando el programa importa
/// `std/alarm`. Atiende las peticiones `alarm_*` del runtime (el canal genérico de M442,
/// `ray_ui_set_request_handler`), programa con `AlarmManager.shared.schedule` y recibe la
/// respuesta del usuario por dos `LiveActivityIntent` (detener y posponer): la alerta la pinta
/// y la hace sonar el SISTEMA, también con la app cerrada. El id de AlarmKit es un UUID; el del
/// programa es texto: el mapa vive en `UserDefaults` junto a los campos de la alarma. Sin cuenta
/// atrás ni Live Activity propia (no hace falta extensión de widget): posponer es otra alarma
/// de una vez. Todo bajo `#available(iOS 26.0, *)`: con un sistema anterior, `"unsupported"`.
const RAY_ALARM_SWIFT: &str = r#"// Generated by `ray bundle --ios` for std/alarm (AlarmKit, iOS 26+).
import ActivityKit
import AlarmKit
import AppIntents
import Foundation
import SwiftUI

// The runtime's C ABI (the static library of the program) and the shell's "has the program
// started?" flag (SceneDelegate.m).
@_silgen_name("ray_ui_request_result")
private func ray_ui_request_result(_ id: Int64, _ value: UnsafePointer<CChar>?, _ err: UnsafePointer<CChar>?)
@_silgen_name("ray_ui_push_event")
private func ray_ui_push_event(_ kind: UnsafePointer<CChar>, _ window: Int64, _ tag: UnsafePointer<CChar>?)
@_silgen_name("ray_shell_started")
private func ray_shell_started() -> Int32

@objc(RayAlarm) public final class RayAlarm: NSObject {
    static let sep = "\u{1f}"
    static let storeKey = "ray_alarm"
    static let responsesKey = "ray_alarm_responses"
    static let lock = NSLock()

    static func ok(_ id: Int64, _ value: String) {
        value.withCString { ray_ui_request_result(id, $0, nil) }
    }

    static func fail(_ id: Int64, _ message: String) {
        message.withCString { ray_ui_request_result(id, nil, $0) }
    }

    /// A request of std/alarm: `alarm_permission`, `alarm_status`, `alarm_schedule`,
    /// `alarm_cancel`, `alarm_cancel_all`, `alarm_pending` (see ray_runtime::alarm).
    @objc public static func handle(_ id: Int64, op: String, arg: String) {
        guard #available(iOS 26.0, *) else {
            if op == "alarm_status" {
                ok(id, "unsupported")
            } else {
                fail(id, "alarm: system alarms need iOS 26 (AlarmKit); std/notify with sound_name rings once")
            }
            return
        }
        Task { await RayAlarmKit.handle(id, op: op, arg: arg) }
    }

    /// The user's answer -> an "alarm" event (`id\taction\tdata`). With the program running, now;
    /// otherwise (the alarm rang with the app closed) it is kept for the next launch.
    static func respond(_ id: String, _ action: String, _ data: String) {
        let tag = "\(id)\t\(action)\t\(data)"
        // The lock orders this against deliverPending: an answer that sees "not started" is
        // stored before the start drains the list, never after.
        lock.lock()
        defer { lock.unlock() }
        if ray_shell_started() != 0 {
            "alarm".withCString { kind in tag.withCString { ray_ui_push_event(kind, 0, $0) } }
        } else {
            var list = UserDefaults.standard.stringArray(forKey: responsesKey) ?? []
            list.append(tag)
            UserDefaults.standard.set(list, forKey: responsesKey)
        }
    }

    /// When the program starts (SceneDelegate, after ray_start): the answers kept meanwhile, in order.
    @objc public static func deliverPending() {
        lock.lock()
        defer { lock.unlock() }
        let list = UserDefaults.standard.stringArray(forKey: responsesKey) ?? []
        if list.isEmpty { return }
        UserDefaults.standard.removeObject(forKey: responsesKey)
        for tag in list {
            "alarm".withCString { kind in tag.withCString { ray_ui_push_event(kind, 0, $0) } }
        }
    }
}

@available(iOS 26.0, *)
struct RayAlarmMeta: AlarmMetadata {}

struct RayAlarmFailure: LocalizedError {
    let message: String
    var errorDescription: String? { message }
}

@available(iOS 26.0, *)
enum RayAlarmKit {
    typealias Store = [String: [String: String]]

    static func store() -> Store {
        (UserDefaults.standard.dictionary(forKey: RayAlarm.storeKey) as? Store) ?? [:]
    }

    static func save(_ s: Store) {
        UserDefaults.standard.set(s, forKey: RayAlarm.storeKey)
    }

    static func statusText() -> String {
        switch AlarmManager.shared.authorizationState {
        case .authorized: return "granted"
        case .denied: return "denied"
        case .notDetermined: return "not_determined"
        @unknown default: return "not_determined"
        }
    }

    static func handle(_ id: Int64, op: String, arg: String) async {
        do {
            switch op {
            case "alarm_status":
                RayAlarm.ok(id, statusText())
            case "alarm_permission":
                do {
                    let state = try await AlarmManager.shared.requestAuthorization()
                    RayAlarm.ok(id, state == .authorized ? "granted" : "denied")
                } catch {
                    // The system cancels the request when the app is not active (launching, or
                    // another app in front); the user's answer, if any, is in the state.
                    let now = statusText()
                    if now != "not_determined" {
                        RayAlarm.ok(id, now)
                    } else {
                        RayAlarm.fail(id, "alarm: the permission request was interrupted (the app must be in the foreground); ask again")
                    }
                }
            case "alarm_schedule":
                RayAlarm.ok(id, try await schedule(arg.components(separatedBy: RayAlarm.sep)))
            case "alarm_cancel":
                cancel(arg.trimmingCharacters(in: .whitespacesAndNewlines))
                RayAlarm.ok(id, "")
            case "alarm_cancel_all":
                for key in store().keys { cancel(key) }
                RayAlarm.ok(id, "")
            case "alarm_pending":
                RayAlarm.ok(id, pending().joined(separator: "\n"))
            default:
                RayAlarm.fail(id, "alarm: unknown op '\(op)'")
            }
        } catch {
            RayAlarm.fail(id, "alarm: \(error.localizedDescription)")
        }
    }

    /// `name[.ext]` -> `name.ext` if the file is in the main bundle (`[ios] sounds`).
    static func soundFile(_ name: String) -> String? {
        if name.isEmpty { return nil }
        let parts = name.split(separator: ".", maxSplits: 1, omittingEmptySubsequences: false)
        let stem = String(parts[0])
        let exts = parts.count > 1 ? [String(parts[1])] : ["caf", "aiff", "aif", "wav", "m4a"]
        for ext in exts where Bundle.main.url(forResource: stem, withExtension: ext) != nil {
            return "\(stem).\(ext)"
        }
        return nil
    }

    /// `at:unix` | `daily:h:m` | `weekly:d,d:h:m` (ISO weekdays: 1 = Monday ... 7 = Sunday).
    static func parseSchedule(_ trigger: String) throws -> Alarm.Schedule {
        let p = trigger.split(separator: ":", omittingEmptySubsequences: false).map(String.init)
        let week: [Locale.Weekday] = [.monday, .tuesday, .wednesday, .thursday, .friday, .saturday, .sunday]
        switch p.first ?? "" {
        case "at":
            guard p.count > 1, let unix = Double(p[1]) else { break }
            return .fixed(Date(timeIntervalSince1970: unix))
        case "daily":
            guard p.count > 2, let h = Int(p[1]), let m = Int(p[2]) else { break }
            return .relative(.init(time: .init(hour: h, minute: m), repeats: .weekly(week)))
        case "weekly":
            guard p.count > 3, let h = Int(p[2]), let m = Int(p[3]) else { break }
            let days = p[1].split(separator: ",").compactMap { Int($0) }.filter { (1...7).contains($0) }.map { week[$0 - 1] }
            if days.isEmpty { break }
            return .relative(.init(time: .init(hour: h, minute: m), repeats: .weekly(days)))
        default:
            break
        }
        throw RayAlarmFailure(message: "bad trigger '\(trigger)'")
    }

    /// The fields of an alarm: trigger, id, title, sound, snooze_minutes, stop_label, data,
    /// max_minutes, snooze_label.
    static func configuration(_ f: [String], uuid: UUID, schedule: Alarm.Schedule) -> AlarmManager.AlarmConfiguration<RayAlarmMeta> {
        let snooze = Int(f[4].trimmingCharacters(in: .whitespaces)) ?? 0
        let stopText = f[5].isEmpty ? "Stop" : f[5]
        let snoozeText = f.count > 8 && !f[8].isEmpty ? f[8] : "Snooze"
        let text = { (s: String) in LocalizedStringResource(String.LocalizationValue(s)) }
        let stop = AlarmButton(text: text(stopText), textColor: .white, systemImageName: "stop.circle")
        let later = snooze > 0 ? AlarmButton(text: text(snoozeText), textColor: .white, systemImageName: "zzz") : nil
        let alert = AlarmPresentation.Alert(title: text(f[2]), stopButton: stop, secondaryButton: later, secondaryButtonBehavior: later == nil ? nil : .custom)
        let attributes = AlarmAttributes<RayAlarmMeta>(presentation: AlarmPresentation(alert: alert), metadata: RayAlarmMeta(), tintColor: .accentColor)
        let sound: AlertConfiguration.AlertSound = soundFile(f[3]).map { .named($0) } ?? .default
        let secondary: (any LiveActivityIntent)? = snooze > 0 ? RayAlarmSnoozeIntent(alarmID: uuid.uuidString) : nil
        return .alarm(schedule: schedule, attributes: attributes, stopIntent: RayAlarmStopIntent(alarmID: uuid.uuidString), secondaryIntent: secondary, sound: sound)
    }

    static func schedule(_ f: [String]) async throws -> String {
        guard f.count >= 8 else { throw RayAlarmFailure(message: "malformed alarm") }
        guard AlarmManager.shared.authorizationState == .authorized else {
            throw RayAlarmFailure(message: "alarms are not authorized for this app (\(statusText()); call request_permission)")
        }
        if !f[3].isEmpty && soundFile(f[3]) == nil {
            throw RayAlarmFailure(message: "sound '\(f[3])' is not in the app bundle (list the file in [ios] sounds of ray.toml and regenerate the shell)")
        }
        let id = f[1]
        cancel(id) // scheduling with an existing id replaces it
        let uuid = UUID()
        _ = try await AlarmManager.shared.schedule(id: uuid, configuration: configuration(f, uuid: uuid, schedule: try parseSchedule(f[0])))
        var s = store()
        s[id] = ["uuid": uuid.uuidString, "fields": f.joined(separator: RayAlarm.sep)]
        save(s)
        return id
    }

    static func cancel(_ id: String) {
        var s = store()
        guard let rec = s[id] else { return }
        for key in ["uuid", "snooze"] {
            if let text = rec[key], let uuid = UUID(uuidString: text) { try? AlarmManager.shared.cancel(id: uuid) }
        }
        s[id] = nil
        save(s)
    }

    /// The armed alarms, reconciled with the system: a one-shot one that already rang is gone.
    static func pending() -> [String] {
        let live = Set(((try? AlarmManager.shared.alarms) ?? []).map { $0.id.uuidString })
        var s = store()
        for (id, rec) in s where !live.contains(rec["uuid"] ?? "") && !live.contains(rec["snooze"] ?? "") {
            s[id] = nil
        }
        save(s)
        return s.keys.sorted()
    }

    /// A button of the system alert (from the intents): `action` is `stop` or `snooze`.
    static func answered(uuid: String, action: String) async {
        var s = store()
        guard let (id, found) = s.first(where: { $0.value["uuid"] == uuid || $0.value["snooze"] == uuid }) else { return }
        var rec = found
        let f = (rec["fields"] ?? "").components(separatedBy: RayAlarm.sep)
        if let alarm = UUID(uuidString: uuid) { try? AlarmManager.shared.stop(id: alarm) }
        let fromSnooze = rec["snooze"] == uuid
        if fromSnooze { rec["snooze"] = nil }
        let minutes = f.count > 4 ? (Int(f[4].trimmingCharacters(in: .whitespaces)) ?? 0) : 0
        if action == "snooze" && minutes > 0 && f.count >= 8 {
            let next = UUID()
            let again = configuration(f, uuid: next, schedule: .fixed(Date().addingTimeInterval(Double(minutes) * 60)))
            if (try? await AlarmManager.shared.schedule(id: next, configuration: again)) != nil { rec["snooze"] = next.uuidString }
        }
        // A one-shot alarm that is neither snoozed nor repeating is done.
        let oneShot = (f.first ?? "").hasPrefix("at:")
        if oneShot && rec["snooze"] == nil { s[id] = nil } else { s[id] = rec }
        save(s)
        RayAlarm.respond(id, action, f.count > 6 ? f[6] : "")
    }
}

@available(iOS 26.0, *)
struct RayAlarmStopIntent: LiveActivityIntent {
    static let title: LocalizedStringResource = "Stop alarm"
    static let openAppWhenRun: Bool = false

    @Parameter(title: "Alarm")
    var alarmID: String

    init() {
        self.alarmID = ""
    }

    init(alarmID: String) {
        self.alarmID = alarmID
    }

    func perform() async throws -> some IntentResult {
        await RayAlarmKit.answered(uuid: alarmID, action: "stop")
        return .result()
    }
}

@available(iOS 26.0, *)
struct RayAlarmSnoozeIntent: LiveActivityIntent {
    static let title: LocalizedStringResource = "Snooze alarm"
    static let openAppWhenRun: Bool = false

    @Parameter(title: "Alarm")
    var alarmID: String

    init() {
        self.alarmID = ""
    }

    init(alarmID: String) {
        self.alarmID = alarmID
    }

    func perform() async throws -> some IntentResult {
        await RayAlarmKit.answered(uuid: alarmID, action: "snooze")
        return .result()
    }
}
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
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ShellOptions {
    pub devtools: bool,
    pub icon: bool,
    pub background_audio: bool,
    /// M447 (pepa #334): el catálogo trae el color `LaunchBackground` (`[ios] background`).
    pub launch_color: bool,
    /// M447 (pepa #336): `[ios] audio_session` — `"playback"` o `"playback_mix"` (MixWithOthers).
    pub audio_session: Option<String>,
    /// M447 (pepa #331): archivos de `[ios] sounds`, copiados a `Shell/` y a la fase Resources.
    pub sounds: Vec<std::path::PathBuf>,
    /// M448b (IDEAS §122): el programa importa `std/alarm` → `RayAlarm.swift` (AlarmKit, iOS 26)
    /// entra al target y el shell atiende las peticiones `alarm_*`.
    pub alarm: bool,
    /// M330 D5: el shell de DESARROLLO registra este esquema URL (su bundle id) y entrega a la
    /// librería de desarrollo (`ray_dev_link`) toda URL con él — el QR de `ray dev --device`
    /// escaneado con la cámara del sistema abre exactamente esta app.
    pub url_scheme: Option<String>,
}

/// El xcconfig: TODO lo afinable vive aquí (el pbxproj solo lo referencia). La elección del
/// `.a` por SDK es la pieza clave: dispositivo y simulador son ambos arm64.
fn xcconfig(name: &str, bundle_id: &str, version: &str, signing: &Signing, opts: &ShellOptions) -> String {
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
    // M403 (rayplay #170): std/media es MPNowPlayingInfoCenter/MPRemoteCommandCenter (MediaPlayer), siempre.
    // M442 (IDEAS §121): std/notify — el AppDelegate es el UNUserNotificationCenterDelegate (UserNotifications).
    let avf = if opts.background_audio { " -framework AVFoundation" } else { "" };
    // M448b: con std/alarm el target lleva un archivo Swift (AlarmKit no existe en Objective-C); el
    // nombre de su cabecera generada se fija para que SceneDelegate.m no dependa del nombre de la app.
    if opts.alarm {
        sign.push_str("SWIFT_VERSION = 5.0\nSWIFT_OBJC_INTERFACE_HEADER_NAME = RayShell-Swift.h\n");
    }
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
         OTHER_LDFLAGS = -lray_app -framework WebKit -framework AudioToolbox -framework MediaPlayer -framework UserNotifications{avf} -lobjc\n\
         {sign}"
    )
}

/// El pbxproj sintético: UN target de app, tres fases (Sources/Frameworks/Resources), configs
/// Debug y Release colgando del xcconfig. UUIDs de 24 hex FIJOS (únicos dentro del archivo — es
/// todo lo que Xcode exige); objectVersion 56 (aceptado por Xcode 14+). M324 (ray808 #15): con
/// icono, `Assets.xcassets` entra en el grupo Shell y en la fase Resources — antes el catálogo se
/// escribía pero nada lo compilaba y la app salía con el icono genérico.
fn pbxproj(name: &str, assets: bool, sounds: &[String], alarm: bool) -> String {
    // M448b: `RayAlarm.swift` (std/alarm) como una fuente más del target.
    let (swift_build, swift_file, swift_source, swift_child) = if alarm {
        (
            "\t\t0000000000000000000000B5 /* RayAlarm.swift in Sources */ = {isa = PBXBuildFile; fileRef = 0000000000000000000000FA /* RayAlarm.swift */; };\n",
            "\t\t0000000000000000000000FA /* RayAlarm.swift */ = {isa = PBXFileReference; lastKnownFileType = sourcecode.swift; path = RayAlarm.swift; sourceTree = \"<group>\"; };\n",
            ", 0000000000000000000000B5",
            ", 0000000000000000000000FA",
        )
    } else {
        ("", "", "", "")
    };
    let (mut res_file, mut res_build, mut res_child, mut res_files) = if assets {
        (
            "\t\t0000000000000000000000F9 /* Assets.xcassets */ = {isa = PBXFileReference; lastKnownFileType = folder.assetcatalog; path = Assets.xcassets; sourceTree = \"<group>\"; };\n".to_string(),
            "\t\t0000000000000000000000B4 /* Assets.xcassets in Resources */ = {isa = PBXBuildFile; fileRef = 0000000000000000000000F9 /* Assets.xcassets */; };\n".to_string(),
            ", 0000000000000000000000F9".to_string(),
            "0000000000000000000000B4".to_string(),
        )
    } else {
        (String::new(), String::new(), String::new(), String::new())
    };
    // M447 (pepa #331): una referencia + un build file por sonido (ids fijos y únicos, como el resto).
    for (i, file) in sounds.iter().enumerate() {
        let fref = format!("00000000000000000000{:04X}", 0x5000 + i);
        let bfile = format!("00000000000000000000{:04X}", 0x6000 + i);
        res_file.push_str(&format!("\t\t{fref} /* {file} */ = {{isa = PBXFileReference; lastKnownFileType = audio; path = \"{file}\"; sourceTree = \"<group>\"; }};\n"));
        res_build.push_str(&format!("\t\t{bfile} /* {file} in Resources */ = {{isa = PBXBuildFile; fileRef = {fref} /* {file} */; }};\n"));
        res_child.push_str(&format!(", {fref}"));
        if !res_files.is_empty() {
            res_files.push_str(", ");
        }
        res_files.push_str(&bfile);
    }
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
{swift_build}{swift_file}{res_build}{res_file}		0000000000000000000000F1 /* main.m */ = {{isa = PBXFileReference; lastKnownFileType = sourcecode.c.objc; path = main.m; sourceTree = "<group>"; }};
		0000000000000000000000F2 /* AppDelegate.h */ = {{isa = PBXFileReference; lastKnownFileType = sourcecode.c.h; path = AppDelegate.h; sourceTree = "<group>"; }};
		0000000000000000000000F3 /* AppDelegate.m */ = {{isa = PBXFileReference; lastKnownFileType = sourcecode.c.objc; path = AppDelegate.m; sourceTree = "<group>"; }};
		0000000000000000000000F4 /* Info.plist */ = {{isa = PBXFileReference; lastKnownFileType = text.plist.xml; path = Info.plist; sourceTree = "<group>"; }};
		0000000000000000000000F7 /* SceneDelegate.h */ = {{isa = PBXFileReference; lastKnownFileType = sourcecode.c.h; path = SceneDelegate.h; sourceTree = "<group>"; }};
		0000000000000000000000F8 /* SceneDelegate.m */ = {{isa = PBXFileReference; lastKnownFileType = sourcecode.c.objc; path = SceneDelegate.m; sourceTree = "<group>"; }};
		0000000000000000000000F5 /* App.xcconfig */ = {{isa = PBXFileReference; lastKnownFileType = text.xcconfig; path = App.xcconfig; sourceTree = "<group>"; }};
		0000000000000000000000F6 /* {name}.app */ = {{isa = PBXFileReference; explicitFileType = wrapper.application; includeInIndex = 0; path = "{name}.app"; sourceTree = BUILT_PRODUCTS_DIR; }};
		0000000000000000000000E1 /* Frameworks */ = {{isa = PBXFrameworksBuildPhase; buildActionMask = 2147483647; files = (); runOnlyForDeploymentPostprocessing = 0; }};
		0000000000000000000000E2 /* Sources */ = {{isa = PBXSourcesBuildPhase; buildActionMask = 2147483647; files = (0000000000000000000000B1, 0000000000000000000000B2, 0000000000000000000000B3{swift_source}); runOnlyForDeploymentPostprocessing = 0; }};
		0000000000000000000000E3 /* Resources */ = {{isa = PBXResourcesBuildPhase; buildActionMask = 2147483647; files = ({res_files}); runOnlyForDeploymentPostprocessing = 0; }};
		0000000000000000000000A1 /* Shell */ = {{isa = PBXGroup; children = (0000000000000000000000F1, 0000000000000000000000F2, 0000000000000000000000F3, 0000000000000000000000F7, 0000000000000000000000F8, 0000000000000000000000F4{res_child}{swift_child}); path = Shell; sourceTree = "<group>"; }};
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
            // M382: el selector de archivos del sistema siempre está en el shell.
            assert!(src.contains("ray_ui_set_pick_handler(ray_pick)") && src.contains("UIDocumentPickerViewController") && src.contains("asCopy:YES"), "picker en el shell");
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
    let ShellOptions { devtools, icon, background_audio, launch_color, ref audio_session, ref sounds, alarm, ref url_scheme } = opts;
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
    // M447 (pepa #336): `[ios] audio_session = "playback_mix"` añade MixWithOthers — abrir la app no
    // corta la música de otra; `"playback"` (o `background_audio` a secas) la interrumpe, como un
    // reproductor. Sin ninguna, la sesión queda en la categoría por defecto (ambient, silencio).
    let mix = audio_session.as_deref() == Some("playback_mix");
    let (audio_import, audio_session) = if background_audio || audio_session.is_some() {
        (
            "#import <AVFoundation/AVFoundation.h>",
            if mix {
                "AVAudioSession *session = [AVAudioSession sharedInstance]; // [ios] audio_session = playback_mix\n    \
                 [session setCategory:AVAudioSessionCategoryPlayback withOptions:AVAudioSessionCategoryOptionMixWithOthers error:nil];\n    \
                 [session setActive:YES error:nil];"
            } else {
                "AVAudioSession *session = [AVAudioSession sharedInstance]; // [ios] background_audio / audio_session = playback\n    \
                 [session setCategory:AVAudioSessionCategoryPlayback error:nil];\n    \
                 [session setActive:YES error:nil];"
            },
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
    // M448b (IDEAS §122): con std/alarm, el shell registra el atendedor de peticiones del runtime
    // (el canal genérico de M442) y las `alarm_*` van a `RayAlarm` (Swift); `ray_shell_started`
    // le dice a Swift si el programa ya corre (si no, la respuesta espera al siguiente arranque).
    let (alarm_shell, alarm_register, alarm_started) = if alarm {
        (
            "// M448b: std/alarm (AlarmKit, iOS 26) — RayAlarm.swift.\n#import \"RayShell-Swift.h\"\nextern void ray_ui_set_request_handler(void (*handler)(long long, const char *, const char *));\nextern void ray_ui_request_result(long long id, const char *value, const char *err);\nstatic int rayStarted = 0;\nint ray_shell_started(void) { return rayStarted; }\nstatic void ray_request(long long id, const char *op, const char *arg) {\n    NSString *o = [NSString stringWithUTF8String:op]; // copiar: válidos solo durante la llamada\n    NSString *a = [NSString stringWithUTF8String:arg];\n    if ([o hasPrefix:@\"alarm_\"]) {\n        [RayAlarm handle:id op:o arg:a];\n    } else {\n        ray_ui_request_result(id, NULL, \"ui: this shell does not answer that request\");\n    }\n}",
            "\n      ray_ui_set_request_handler(ray_request); // M448b",
            "\n      rayStarted = 1;\n      [RayAlarm deliverPending]; // M448b: alarmas respondidas con la app cerrada",
        )
    } else {
        ("", "", "")
    };
    {
        let (dev_extern, dev_connect, dev_method) = match url_scheme {
            Some(_) => (
                "extern void ray_dev_link(const char *url); // Enlace por el esquema URL del shell de desarrollo",
                "for (UIOpenURLContext *ctx in connectionOptions.URLContexts) { ray_dev_link(ctx.URL.absoluteString.UTF8String); }",
                "// La app ya abierta recibe un enlace (QR escaneado con la cámara del sistema).\n- (void)scene:(UIScene *)scene openURLContexts:(NSSet<UIOpenURLContext *> *)URLContexts {\n    for (UIOpenURLContext *ctx in URLContexts) { ray_dev_link(ctx.URL.absoluteString.UTF8String); }\n}",
            ),
            None => ("", "", ""),
        };
        write("Shell/SceneDelegate.m", &SCENE_DELEGATE_M.replace("/*RAY_DEVTOOLS*/", devtools_line).replace("/*RAY_DEV_EXTERN*/", dev_extern).replace("/*RAY_DEV_LINK_CONNECT*/", dev_connect).replace("/*RAY_DEV_LINK_METHOD*/", dev_method).replace("/*RAY_ALARM_SHELL*/", alarm_shell).replace(" /*RAY_ALARM_REGISTER*/", alarm_register).replace(" /*RAY_ALARM_STARTED*/", alarm_started))?;
    }
    // M447 (pepa #334): la pantalla de arranque usa el color `LaunchBackground` del catálogo (con
    // su variante oscura) en vez del blanco del sistema.
    let launch_screen = |p: String| {
        if launch_color {
            p.replace("<key>UILaunchScreen</key><dict/>", "<key>UILaunchScreen</key><dict><key>UIColorName</key><string>LaunchBackground</string></dict>")
        } else {
            p
        }
    };
    // M330 D5: el shell de desarrollo registra su esquema URL (CFBundleURLTypes).
    let plist = match url_scheme {
        Some(scheme) => info_plist(plist_extra).replacen(
            "</dict>\n</plist>",
            &format!(
                "  <key>CFBundleURLTypes</key>\n  <array>\n    <dict>\n      <key>CFBundleURLName</key><string>{scheme}</string>\n      <key>CFBundleURLSchemes</key><array><string>{scheme}</string></array>\n    </dict>\n  </array>\n</dict>\n</plist>"
            ),
            1,
        ),
        None => info_plist(plist_extra),
    };
    let plist = launch_screen(plist);
    write("Shell/Info.plist", &plist)?;
    write("App.xcconfig", &xcconfig(name, bundle_id, version, signing, &opts))?;
    // M447 (pepa #331): los sonidos van a Shell/ y a la fase Resources (cada uno con su referencia).
    let mut sound_names = Vec::new();
    for src in sounds {
        let file = src.file_name().map(|f| f.to_string_lossy().into_owned()).unwrap_or_default();
        std::fs::copy(src, dir.join("Shell").join(&file)).map_err(|e| format!("could not copy the sound {}: {e}", src.display()))?;
        sound_names.push(file);
    }
    if alarm {
        write("Shell/RayAlarm.swift", RAY_ALARM_SWIFT)?;
    }
    write(&format!("{name}.xcodeproj/project.pbxproj"), &pbxproj(name, icon || launch_color, &sound_names, alarm))?;
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

    /// rayround #182: los errores de la página salen por stderr sin --devtools.
    #[test]
    fn the_shell_forwards_page_errors_to_stderr() {
        assert!(SCENE_DELEGATE_M.contains("messageHandlers.rayLog.postMessage(String(s))"));
        assert!(SCENE_DELEGATE_M.contains("addScriptMessageHandler:[RayLogHandler new] name:@\"rayLog\""));
        assert!(SCENE_DELEGATE_M.contains("initWithSource:rayLogShim"));
        assert!(SCENE_DELEGATE_M.contains("addEventListener('error'"));
        assert!(SCENE_DELEGATE_M.contains("addEventListener('unhandledrejection'"));
        assert!(SCENE_DELEGATE_M.contains("fprintf(stderr, \"%s\\n\""));
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
        // pepa #328: el viewport mide la pantalla entera; las zonas seguras son de la página.
        assert!(SCENE_DELEGATE_M.contains("rayWebView.scrollView.contentInsetAdjustmentBehavior = UIScrollViewContentInsetAdjustmentNever;"));
        assert!(SCENE_DELEGATE_M.contains("ray_ui_shell_capabilities(1);\n      ray_start();"), "capabilities before ray_start");
        assert!(SCENE_DELEGATE_M.contains("setenv(\"RAY_DEV_APP_NAME\""), "M343: the app label reaches the dev library");
        assert!(SCENE_DELEGATE_M.contains("if (![self->_stopped containsObject:key]) {\n            [task didFinish];"));
    }

    /// M324 (ray808 #15): con icono, el catálogo entra en el pbxproj (grupo + fase Resources) y el
    /// xcconfig nombra el AppIcon; sin icono, ni rastro (un catálogo referenciado y ausente rompe el build).
    #[test]
    fn the_icon_is_wired_into_the_project_only_when_present() {
        let with = pbxproj("Demo", true, &[], false);
        assert!(with.contains("PBXResourcesBuildPhase; buildActionMask = 2147483647; files = (0000000000000000000000B4)"), "{with}");
        assert!(with.contains("path = Assets.xcassets; sourceTree"), "{with}");
        assert!(with.contains("0000000000000000000000F4, 0000000000000000000000F9); path = Shell"), "{with}");
        assert!(with.contains("buildPhases = (0000000000000000000000E2, 0000000000000000000000E1, 0000000000000000000000E3)"), "{with}");
        let without = pbxproj("Demo", false, &[], false);
        assert!(!without.contains("Assets.xcassets"), "{without}");
        assert!(without.contains("PBXResourcesBuildPhase; buildActionMask = 2147483647; files = ()"), "{without}");
        let opts = ShellOptions { icon: true, ..Default::default() };
        assert!(xcconfig("Demo", "org.raylang.demo", "1.0.0", &Signing::default(), &opts).contains("ASSETCATALOG_COMPILER_APPICON_NAME = AppIcon\n"));
        assert!(!xcconfig("Demo", "org.raylang.demo", "1.0.0", &Signing::default(), &ShellOptions::default()).contains("ASSETCATALOG"));
    }

    /// M447 (pepa #334/#336/#331): color de arranque en el plist y la escena, webview transparente
    /// y fondo leído de `ray_ui_open_background`; `audio_session = "playback_mix"` añade
    /// MixWithOthers; los sonidos entran en Shell/ y en la fase Resources.
    #[test]
    fn launch_color_audio_mix_and_sounds_reach_the_shell() {
        let base = std::env::temp_dir().join(format!("ray_ios_m447_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let snd = base.join("alarm.caf");
        std::fs::create_dir_all(&base).unwrap();
        std::fs::write(&snd, b"caff").unwrap();
        let opts = ShellOptions { launch_color: true, audio_session: Some("playback_mix".to_string()), sounds: vec![snd.clone()], ..Default::default() };
        write_project(&base, "App", "org.example.app", "1.0.0", &Signing::default(), opts, &[]).unwrap();
        let plist = std::fs::read_to_string(base.join("Shell/Info.plist")).unwrap();
        assert!(plist.contains("<key>UILaunchScreen</key><dict><key>UIColorName</key><string>LaunchBackground</string></dict>"), "{plist}");
        let app = std::fs::read_to_string(base.join("Shell/AppDelegate.m")).unwrap();
        assert!(app.contains("withOptions:AVAudioSessionCategoryOptionMixWithOthers error:nil]"), "{app}");
        assert!(!app.contains("[session setCategory:AVAudioSessionCategoryPlayback error:nil];"));
        let pbx = std::fs::read_to_string(base.join("App.xcodeproj/project.pbxproj")).unwrap();
        assert!(pbx.contains("/* alarm.caf */ = {isa = PBXFileReference; lastKnownFileType = audio; path = \"alarm.caf\""), "{pbx}");
        assert!(pbx.contains("/* alarm.caf in Resources */") && pbx.contains("/* Assets.xcassets in Resources */"), "{pbx}");
        assert!(base.join("Shell/alarm.caf").exists());
        // Sin color: el plist por defecto; con `playback`: la sesión sin mezcla.
        let _ = std::fs::remove_dir_all(&base);
        let opts = ShellOptions { audio_session: Some("playback".to_string()), ..Default::default() };
        write_project(&base, "App", "org.example.app", "1.0.0", &Signing::default(), opts, &[]).unwrap();
        assert!(std::fs::read_to_string(base.join("Shell/Info.plist")).unwrap().contains("<key>UILaunchScreen</key><dict/>"));
        let app = std::fs::read_to_string(base.join("Shell/AppDelegate.m")).unwrap();
        assert!(app.contains("[session setCategory:AVAudioSessionCategoryPlayback error:nil];") && !app.contains("MixWithOthers"));
        assert!(!std::fs::read_to_string(base.join("App.xcodeproj/project.pbxproj")).unwrap().contains("Assets.xcassets"));
        let _ = std::fs::remove_dir_all(&base);
        // La escena: color de arranque, webview transparente y el fondo del programa al abrir.
        for s in ["[UIColor colorNamed:@\"LaunchBackground\"] ?: UIColor.systemBackgroundColor", "rayWebView.opaque = NO;", "rayColorFromHex(ray_ui_open_background())", "extern const char *ray_ui_open_background(void);", "rayLastBackground = bg;", "UIColor *launch = rayLastBackground ?:"] {
            assert!(SCENE_DELEGATE_M.contains(s), "{s}");
        }
    }

    /// M448b (IDEAS §122): `RayAlarm.swift` (AlarmKit) entra al proyecto SOLO con std/alarm: la
    /// fuente Swift en el pbxproj, los ajustes de Swift en el xcconfig y, en la escena, el
    /// atendedor de peticiones registrado antes de `ray_start` y las respuestas guardadas
    /// entregadas después. Sin alarma, ni rastro de Swift.
    #[test]
    fn the_alarm_swift_piece_is_wired_only_with_std_alarm() {
        let base = std::env::temp_dir().join(format!("ray_ios_alarm_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let opts = ShellOptions { alarm: true, ..Default::default() };
        write_project(&base, "App", "org.example.app", "1.0.0", &Signing::default(), opts, &[]).unwrap();
        let swift = std::fs::read_to_string(base.join("Shell/RayAlarm.swift")).unwrap();
        for needle in [
            "@objc(RayAlarm) public final class RayAlarm: NSObject",
            "guard #available(iOS 26.0, *) else {",
            "try await AlarmManager.shared.requestAuthorization()",
            "try await AlarmManager.shared.schedule(id: uuid, configuration:",
            "stopIntent: RayAlarmStopIntent(alarmID: uuid.uuidString)",
            "struct RayAlarmSnoozeIntent: LiveActivityIntent",
            "@_silgen_name(\"ray_ui_request_result\")",
            "UserDefaults.standard.set(list, forKey: responsesKey)",
        ] {
            assert!(swift.contains(needle), "{needle}");
        }
        let pbx = std::fs::read_to_string(base.join("App.xcodeproj/project.pbxproj")).unwrap();
        assert!(pbx.contains("/* RayAlarm.swift */ = {isa = PBXFileReference; lastKnownFileType = sourcecode.swift;"), "{pbx}");
        assert!(pbx.contains("0000000000000000000000B3, 0000000000000000000000B5); runOnlyForDeploymentPostprocessing"), "{pbx}");
        let cfg = std::fs::read_to_string(base.join("App.xcconfig")).unwrap();
        assert!(cfg.contains("SWIFT_VERSION = 5.0") && cfg.contains("SWIFT_OBJC_INTERFACE_HEADER_NAME = RayShell-Swift.h"), "{cfg}");
        let scene = std::fs::read_to_string(base.join("Shell/SceneDelegate.m")).unwrap();
        assert!(scene.contains("#import \"RayShell-Swift.h\"") && scene.contains("int ray_shell_started(void) { return rayStarted; }"));
        let (register, start, deliver) = (scene.find("ray_ui_set_request_handler(ray_request);").unwrap(), scene.find("ray_start();").unwrap(), scene.find("[RayAlarm deliverPending];").unwrap());
        assert!(register < start && start < deliver, "registrar antes de arrancar, entregar después");
        assert!(!scene.contains("/*RAY_ALARM"), "sin marcadores");
        // Sin std/alarm: el shell de siempre, en Objective-C.
        let _ = std::fs::remove_dir_all(&base);
        write_project(&base, "App", "org.example.app", "1.0.0", &Signing::default(), ShellOptions::default(), &[]).unwrap();
        assert!(!base.join("Shell/RayAlarm.swift").exists());
        let scene = std::fs::read_to_string(base.join("Shell/SceneDelegate.m")).unwrap();
        assert!(!scene.contains("RayAlarm") && !scene.contains("/*RAY_ALARM") && scene.contains("ray_ui_shell_capabilities(1);\n      ray_start();\n"), "{scene}");
        assert!(!std::fs::read_to_string(base.join("App.xcodeproj/project.pbxproj")).unwrap().contains("swift"));
        assert!(!std::fs::read_to_string(base.join("App.xcconfig")).unwrap().contains("SWIFT_"));
        let _ = std::fs::remove_dir_all(&base);
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

    /// M442 (IDEAS §121): el AppDelegate es el delegate del centro de notificaciones desde el
    /// arranque (pulsación que abre la app incluida), reenvía las pulsaciones como evento
    /// `"notification"` con el tag de `notify.event_tap`, y el xcconfig enlaza UserNotifications.
    #[test]
    fn the_shell_delegates_user_notifications_to_the_program() {
        assert!(APP_DELEGATE_M.contains("[UNUserNotificationCenter currentNotificationCenter].delegate = self;"));
        assert!(APP_DELEGATE_M.contains("didReceiveNotificationResponse:(UNNotificationResponse *)response"));
        assert!(APP_DELEGATE_M.contains("ray_ui_push_event(\"notification\", 0, tag.UTF8String);"));
        assert!(APP_DELEGATE_M.contains("UNNotificationPresentationOptionBanner | UNNotificationPresentationOptionList"));
        let cfg = xcconfig("App", "org.example.app", "1.0.0", &Signing::default(), &ShellOptions::default());
        assert!(cfg.contains("-framework UserNotifications"), "{cfg}");
    }

    #[test]
    fn the_xcconfig_carries_the_resolved_signing() {
        let s = Signing { team: Some("ABC123".into()), style: Some("Automatic".into()) };
        let text = xcconfig("Demo", "org.raylang.demo", "1.0.0", &s, &ShellOptions::default());
        assert!(text.contains("DEVELOPMENT_TEAM = ABC123"), "{text}");
        assert!(text.contains("CODE_SIGN_STYLE = Automatic"), "{text}");
        // Y el round-trip: lo que el bundle escribe, la siguiente regeneración lo preserva.
        let back = Signing::from_xcconfig(&text);
        assert_eq!(back.team.as_deref(), Some("ABC123"));
        let bare = xcconfig("Demo", "org.raylang.demo", "1.0.0", &Signing::default(), &ShellOptions::default());
        assert!(!bare.contains("DEVELOPMENT_TEAM"), "{bare}");
    }

    /// M330 D5: el shell de desarrollo registra su bundle id como esquema URL y entrega a la
    /// librería (`ray_dev_link`) las URLs que llegan al conectar la escena o con la app abierta.
    #[test]
    fn the_dev_shell_registers_its_url_scheme_and_hands_links_to_the_library() {
        let base = std::env::temp_dir().join(format!("ray-ios-devlink-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&base).unwrap();
        let opts = ShellOptions { devtools: true, url_scheme: Some("org.example.app.dev".to_string()), ..Default::default() };
        write_project(&base, "App", "org.example.app.dev", "1.0.0", &Signing::default(), opts, &[]).unwrap();
        let plist = std::fs::read_to_string(base.join("Shell/Info.plist")).unwrap();
        assert!(plist.contains("<key>CFBundleURLSchemes</key><array><string>org.example.app.dev</string></array>"));
        let scene = std::fs::read_to_string(base.join("Shell/SceneDelegate.m")).unwrap();
        assert!(scene.contains("extern void ray_dev_link(const char *url);"));
        assert!(scene.contains("connectionOptions.URLContexts"));
        assert!(scene.contains("openURLContexts:(NSSet<UIOpenURLContext *> *)URLContexts"));
        write_project(&base, "App", "org.example.app", "1.0.0", &Signing::default(), ShellOptions::default(), &[]).unwrap();
        let plist = std::fs::read_to_string(base.join("Shell/Info.plist")).unwrap();
        assert!(!plist.contains("CFBundleURLTypes"));
        let scene = std::fs::read_to_string(base.join("Shell/SceneDelegate.m")).unwrap();
        assert!(!scene.contains("ray_dev_link"));
        let _ = std::fs::remove_dir_all(&base);
    }
}
