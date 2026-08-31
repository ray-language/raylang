//! §80b — `ray bundle --ios`: el proyecto Xcode GENERADO de una app iOS. La app es un SHELL
//! delgado en Objective-C (UIWindow + WKWebView a pantalla completa) que linkea el staticlib
//! del programa (`ray build --native --lib`): registra sus handlers con `ray_ui_set_handlers`,
//! llama `ray_start()` y desde ahí el programa raylang manda — su webserver embebido sirve la
//! UI y `ui.open(title, url)` le entrega la URL al webview. MISMO FUENTE que el escritorio.
//!
//! Decisiones de plantilla (revisadas en el plan): xcconfig POR SDK para elegir el `.a`
//! (dispositivo y simulador son AMBOS arm64 — un lipo es imposible; el xcframework queda para
//! v2), pbxproj MÍNIMO con UUIDs sintéticos (24 hex, únicos en el archivo) y objectVersion 56,
//! todo lo afinable en el xcconfig; sin escenas (UIWindow clásico: sin UIApplicationSceneManifest
//! el camino AppDelegate sigue vigente); firma: el simulador no la necesita (el smoke compila
//! con CODE_SIGNING_ALLOWED=NO); dispositivo = abrir en Xcode y elegir team (documentado).

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

/// El corazón del shell: registra los handlers ANTES de ray_start (contrato del staticlib:
/// strings NUL-terminated válidos solo durante la llamada → se COPIAN antes de despachar al
/// hilo principal, que es donde WebKit exige vivir).
const APP_DELEGATE_M: &str = r#"#import "AppDelegate.h"
#import <WebKit/WebKit.h>

extern void ray_ui_set_handlers(void (*open)(const char *, const char *),
                                void (*eval)(const char *));
extern void ray_ui_push_event(const char *kind, long long window, const char *tag);
extern int ray_start(void);

static WKWebView *rayWebView = nil;

static void ray_open(const char *title, const char *url) {
    NSString *u = [NSString stringWithUTF8String:url]; // copiar ANTES de despachar
    dispatch_async(dispatch_get_main_queue(), ^{
      [rayWebView loadRequest:[NSURLRequest requestWithURL:[NSURL URLWithString:u]]];
    });
}

static void ray_eval(const char *js) {
    NSString *s = [NSString stringWithUTF8String:js];
    dispatch_async(dispatch_get_main_queue(), ^{
      [rayWebView evaluateJavaScript:s completionHandler:nil];
    });
}

@implementation AppDelegate

- (BOOL)application:(UIApplication *)application
    didFinishLaunchingWithOptions:(NSDictionary *)launchOptions {
    self.window = [[UIWindow alloc] initWithFrame:UIScreen.mainScreen.bounds];
    UIViewController *vc = [UIViewController new];
    rayWebView = [[WKWebView alloc] initWithFrame:vc.view.bounds];
    rayWebView.autoresizingMask =
        UIViewAutoresizingFlexibleWidth | UIViewAutoresizingFlexibleHeight;
    [vc.view addSubview:rayWebView];
    self.window.rootViewController = vc;
    [self.window makeKeyAndVisible];
    ray_ui_set_handlers(ray_open, ray_eval);
    ray_start();
    return YES;
}

- (void)applicationDidEnterBackground:(UIApplication *)application {
    ray_ui_push_event("lifecycle", 0, "background");
}

- (void)applicationWillEnterForeground:(UIApplication *)application {
    ray_ui_push_event("lifecycle", 0, "foreground");
}

@end
"#;

/// Info.plist del shell (placeholders de build settings; `UILaunchScreen` vacío = pantalla de
/// lanzamiento por defecto sin storyboard; ATS con local networking como en el .app de mac).
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
  <key>NSAppTransportSecurity</key>
  <dict><key>NSAllowsLocalNetworking</key><true/></dict>
</dict>
</plist>
"#;

/// El xcconfig: TODO lo afinable vive aquí (el pbxproj solo lo referencia). La elección del
/// `.a` por SDK es la pieza clave: dispositivo y simulador son ambos arm64.
fn xcconfig(name: &str, bundle_id: &str, version: &str) -> String {
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
         OTHER_LDFLAGS = -lray_app -framework WebKit -lobjc\n"
    )
}

/// El pbxproj sintético: UN target de app, dos fases (Sources/Frameworks), configs Debug y
/// Release colgando del xcconfig. UUIDs de 24 hex FIJOS (únicos dentro del archivo — es todo
/// lo que Xcode exige); objectVersion 56 (aceptado por Xcode 14+).
fn pbxproj(name: &str) -> String {
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
		0000000000000000000000F1 /* main.m */ = {{isa = PBXFileReference; lastKnownFileType = sourcecode.c.objc; path = main.m; sourceTree = "<group>"; }};
		0000000000000000000000F2 /* AppDelegate.h */ = {{isa = PBXFileReference; lastKnownFileType = sourcecode.c.h; path = AppDelegate.h; sourceTree = "<group>"; }};
		0000000000000000000000F3 /* AppDelegate.m */ = {{isa = PBXFileReference; lastKnownFileType = sourcecode.c.objc; path = AppDelegate.m; sourceTree = "<group>"; }};
		0000000000000000000000F4 /* Info.plist */ = {{isa = PBXFileReference; lastKnownFileType = text.plist.xml; path = Info.plist; sourceTree = "<group>"; }};
		0000000000000000000000F5 /* App.xcconfig */ = {{isa = PBXFileReference; lastKnownFileType = text.xcconfig; path = App.xcconfig; sourceTree = "<group>"; }};
		0000000000000000000000F6 /* {name}.app */ = {{isa = PBXFileReference; explicitFileType = wrapper.application; includeInIndex = 0; path = "{name}.app"; sourceTree = BUILT_PRODUCTS_DIR; }};
		0000000000000000000000E1 /* Frameworks */ = {{isa = PBXFrameworksBuildPhase; buildActionMask = 2147483647; files = (); runOnlyForDeploymentPostprocessing = 0; }};
		0000000000000000000000E2 /* Sources */ = {{isa = PBXSourcesBuildPhase; buildActionMask = 2147483647; files = (0000000000000000000000B1, 0000000000000000000000B2); runOnlyForDeploymentPostprocessing = 0; }};
		0000000000000000000000A1 /* Shell */ = {{isa = PBXGroup; children = (0000000000000000000000F1, 0000000000000000000000F2, 0000000000000000000000F3, 0000000000000000000000F4); path = Shell; sourceTree = "<group>"; }};
		0000000000000000000000A2 /* Products */ = {{isa = PBXGroup; children = (0000000000000000000000F6); name = Products; sourceTree = "<group>"; }};
		0000000000000000000000A3 = {{isa = PBXGroup; children = (0000000000000000000000A1, 0000000000000000000000F5, 0000000000000000000000A2); sourceTree = "<group>"; }};
		0000000000000000000000D1 /* {name} */ = {{isa = PBXNativeTarget; buildConfigurationList = 0000000000000000000000C3; buildPhases = (0000000000000000000000E2, 0000000000000000000000E1); buildRules = (); dependencies = (); name = "{name}"; productName = "{name}"; productReference = 0000000000000000000000F6; productType = "com.apple.product-type.application"; }};
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

const README: &str = r#"# App iOS generada por `ray bundle --ios`

- `libs/` y `libs-sim/` llevan el staticlib del programa (dispositivo / simulador); el
  xcconfig elige por SDK. Para regenerarlos tras cambiar el programa:
  `ray bundle --ios` de nuevo (o `ray build --native --lib --target aarch64-apple-ios…`).
- Simulador (sin firma):
  `xcodebuild -project <Name>.xcodeproj -target <Name> -sdk iphonesimulator -configuration Debug build CODE_SIGNING_ALLOWED=NO`
  y luego `xcrun simctl boot <device>` + `install` + `launch`.
- Dispositivo: abrir el `.xcodeproj` en Xcode y elegir tu equipo de firma (Signing & Teams).
- El programa raylang corre DENTRO de la app (staticlib): su webserver embebido sirve la UI y
  `ui.open(title, url)` carga la URL en el webview. Los eventos de ciclo de vida llegan por
  `ui.next_event()` como kind="lifecycle", tag="background"/"foreground".
"#;

/// Genera el árbol del proyecto en `dir` (ya creado). Los `.a` los copia el llamador.
pub fn write_project(dir: &Path, name: &str, bundle_id: &str, version: &str) -> Result<(), String> {
    let write = |rel: &str, content: &str| -> Result<(), String> {
        let p = dir.join(rel);
        if let Some(parent) = p.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        std::fs::write(&p, content).map_err(|e| format!("{}: {e}", p.display()))
    };
    write("Shell/main.m", MAIN_M)?;
    write("Shell/AppDelegate.h", APP_DELEGATE_H)?;
    write("Shell/AppDelegate.m", APP_DELEGATE_M)?;
    write("Shell/Info.plist", INFO_PLIST)?;
    write("App.xcconfig", &xcconfig(name, bundle_id, version))?;
    write(&format!("{name}.xcodeproj/project.pbxproj"), &pbxproj(name))?;
    write("README.md", README)?;
    Ok(())
}
