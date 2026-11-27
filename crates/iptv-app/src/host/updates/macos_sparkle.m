// Sparkle 2.10.1's programmatic API, loaded only from our sealed app bundle.
// Keep method declarations in step with the pinned upstream public headers.
#import <AppKit/AppKit.h>
#import <Security/Security.h>
#include <stdbool.h>
#include <stdint.h>

@protocol PatinaSparkleUpdater <NSObject>
@property BOOL automaticallyChecksForUpdates;
@property BOOL automaticallyDownloadsUpdates;
@property(readonly) BOOL sessionInProgress;
- (BOOL)startUpdater:(NSError **)error;
- (void)checkForUpdates;
@end

@protocol PatinaSparkleController <NSObject>
- (instancetype)initWithStartingUpdater:(BOOL)start updaterDelegate:(id)delegate userDriverDelegate:(id)driver;
@property(readonly) id<PatinaSparkleUpdater> updater;
@property(readonly) id userDriver;
@end

@protocol PatinaSparkleDriver <NSObject>
- (void)dismissUpdateInstallation;
@end

static BOOL PatinaSparkleConfiguration(NSDictionary *info) {
    if (![info[@"PatinaSparkleEnabled"] isEqual:@YES] ||
        ![info[@"SUEnableAutomaticChecks"] isEqual:@NO] ||
        ![info[@"SUAllowsAutomaticUpdates"] isEqual:@NO]) return NO;
    id feed = info[@"SUFeedURL"], key = info[@"SUPublicEDKey"];
    if (![feed isKindOfClass:NSString.class] || ![key isKindOfClass:NSString.class]) return NO;
    NSURLComponents *url = [NSURLComponents componentsWithString:feed];
    NSData *publicKey = [[NSData alloc] initWithBase64EncodedString:key options:0];
    return [url.scheme isEqual:@"https"] && url.host.length &&
        !url.user && !url.password && !url.fragment && publicKey.length == 32;
}

static BOOL PatinaDeveloperIDProcess(void) {
    SecCodeRef code = NULL;
    SecRequirementRef requirement = NULL;
    // A locally ad-hoc signed preview is intentionally not self-updating.
    // Validate the running code, without walking/hashing the entire media
    // runtime and resource tree synchronously on the UI thread.
    OSStatus status = SecCodeCopySelf(kSecCSDefaultFlags, &code);
    if (status == errSecSuccess) {
        status = SecRequirementCreateWithString(CFSTR("anchor apple generic and certificate leaf[field.1.2.840.113635.100.6.1.13] exists"),
                                                kSecCSDefaultFlags, &requirement);
    }
    if (status == errSecSuccess) {
        status = SecCodeCheckValidity(code, kSecCSDefaultFlags, requirement);
    }
    if (requirement) CFRelease(requirement);
    if (code) CFRelease(code);
    return status == errSecSuccess;
}

bool patina_sparkle_supported(void) {
    NSBundle *app = NSBundle.mainBundle;
    if (!PatinaSparkleConfiguration(app.infoDictionary)) return false;
    NSURL *path = [app.privateFrameworksURL URLByAppendingPathComponent:@"Sparkle.framework"];
    NSBundle *framework = [NSBundle bundleWithURL:path];
    return [framework.bundleIdentifier isEqual:@"org.sparkle-project.Sparkle"] &&
        [framework.infoDictionary[@"CFBundleShortVersionString"] isEqual:@"2.10.1"] &&
        PatinaDeveloperIDProcess();
}

@interface PatinaSparkleDelegate : NSObject
@property(copy) NSString *expectedVersion;
@property uint64_t session;
@property(copy) void (^restartHandler)(void);
@end

// Only this mailbox crosses NSH's shell thread and AppKit's main thread.
// Status: 0 absent, 1 starting, 2 active, 3 restart requested, 4 finished,
// 5 failed, 6 restart authorized. Runtime/Ui pointers never cross that boundary.
static uint64_t PatinaSession;
static uint64_t PatinaNextSession;
static uint8_t PatinaStatus;

static BOOL PatinaMatches(uint64_t session) {
    @synchronized(PatinaSparkleDelegate.class) { return PatinaSession == session; }
}

static void PatinaPublish(uint64_t session, uint8_t status) {
    @synchronized(PatinaSparkleDelegate.class) {
        if (PatinaSession == session) PatinaStatus = status;
    }
}

@implementation PatinaSparkleDelegate
- (BOOL)updater:(id)updater shouldProceedWithUpdate:(id)item updateCheck:(NSInteger)check error:(NSError **)error {
    // GitHub discovery and the authenticated appcast must agree about the
    // version the viewer chose. Never install an unrelated feed version.
    NSString *version = [item valueForKey:@"displayVersionString"];
    if (PatinaMatches(self.session) && [version isEqual:self.expectedVersion]) return YES;
    if (error) *error = [NSError errorWithDomain:@"PatinaUpdate" code:1
        userInfo:@{NSLocalizedDescriptionKey: NSLocalizedString(@"The installer could not be verified or opened.", nil)}];
    return NO;
}
- (BOOL)updater:(id)updater shouldPostponeRelaunchForUpdate:(id)item untilInvokingBlock:(void (^)(void))install {
    // Always ask the owning shell task, even if no recording was active when
    // installation began. Disposed roots cannot authorize a later restart.
    if (PatinaMatches(self.session)) {
        self.restartHandler = install;
        PatinaPublish(self.session, 3);
    }
    return YES;
}
- (void)updater:(id)updater didFinishUpdateCycleForUpdateCheck:(NSInteger)check error:(NSError *)error {
    if (!self.restartHandler) PatinaPublish(self.session, error ? 5 : 4);
}
@end

static id<PatinaSparkleController> PatinaUpdaterController;
static PatinaSparkleDelegate *PatinaUpdaterDelegate;

uint8_t patina_sparkle_status(uint64_t session) {
    @synchronized(PatinaSparkleDelegate.class) { return PatinaSession == session ? PatinaStatus : 0; }
}

static uint64_t PatinaStart(NSString *expected) {
    uint64_t session;
    @synchronized(PatinaSparkleDelegate.class) {
        if (PatinaSession != 0) return 0;
        session = ++PatinaNextSession;
        PatinaSession = session;
        PatinaStatus = 1;
    }
    dispatch_async(dispatch_get_main_queue(), ^{
        if (!PatinaMatches(session)) return;
        if (PatinaUpdaterController.updater.sessionInProgress) { PatinaPublish(session, 5); return; }
        if (!PatinaUpdaterController) {
        NSURL *path = [NSBundle.mainBundle.privateFrameworksURL URLByAppendingPathComponent:@"Sparkle.framework"];
        NSBundle *framework = [NSBundle bundleWithURL:path];
        if (![framework loadAndReturnError:NULL]) { PatinaPublish(session, 5); return; }
        Class controller = NSClassFromString(@"SPUStandardUpdaterController");
        if (!controller) { PatinaPublish(session, 5); return; }
        PatinaUpdaterDelegate = [PatinaSparkleDelegate new];
        PatinaUpdaterController = [(id<PatinaSparkleController>)[controller alloc]
            initWithStartingUpdater:NO updaterDelegate:PatinaUpdaterDelegate userDriverDelegate:nil];
        PatinaUpdaterController.updater.automaticallyChecksForUpdates = NO;
        PatinaUpdaterController.updater.automaticallyDownloadsUpdates = NO;
        if (![PatinaUpdaterController.updater startUpdater:NULL]) {
            PatinaUpdaterController = nil;
            PatinaUpdaterDelegate = nil;
            PatinaPublish(session, 5);
            return;
        }
        }
        PatinaUpdaterDelegate.session = session;
        PatinaUpdaterDelegate.expectedVersion = expected;
        PatinaPublish(session, 2);
        [PatinaUpdaterController.updater checkForUpdates];
    });
    return session;
}

uint64_t patina_sparkle_start(const char *version) {
    if (!version || !patina_sparkle_supported()) return 0;
    NSString *expected = [NSString stringWithUTF8String:version];
    return expected.length ? PatinaStart(expected) : 0;
}

void patina_sparkle_allow_restart(uint64_t session) {
    @synchronized(PatinaSparkleDelegate.class) {
        if (PatinaSession != session || PatinaStatus != 3) return;
        PatinaStatus = 6;
    }
    dispatch_async(dispatch_get_main_queue(), ^{
        if (!PatinaMatches(session) || PatinaUpdaterDelegate.session != session) return;
        void (^install)(void) = PatinaUpdaterDelegate.restartHandler;
        PatinaUpdaterDelegate.restartHandler = nil;
        if (install) install();
    });
}

void patina_sparkle_cancel(uint64_t session) {
    @synchronized(PatinaSparkleDelegate.class) {
        if (PatinaSession != session) return;
        PatinaSession = 0;
    }
    dispatch_async(dispatch_get_main_queue(), ^{
        if (PatinaUpdaterDelegate.session != session) return;
        PatinaUpdaterDelegate.restartHandler = nil;
        id<PatinaSparkleDriver> driver = PatinaUpdaterController.userDriver;
        if ([driver respondsToSelector:@selector(dismissUpdateInstallation)]) [driver dismissUpdateInstallation];
    });
}
