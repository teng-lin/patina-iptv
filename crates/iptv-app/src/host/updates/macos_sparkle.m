// Sparkle 2.10.1's programmatic API, loaded only from our sealed app bundle.
// Keep method declarations in step with the pinned upstream public headers.
#import <AppKit/AppKit.h>
#import <Security/Security.h>
#include <assert.h>
#include <stdbool.h>
#include <stdint.h>

@protocol PatinaSparkleUpdater <NSObject>
@property BOOL automaticallyChecksForUpdates;
@property BOOL automaticallyDownloadsUpdates;
@property(readonly) BOOL sessionInProgress;
- (instancetype)initWithHostBundle:(NSBundle *)host applicationBundle:(NSBundle *)application userDriver:(id)driver delegate:(id)delegate;
- (BOOL)startUpdater:(NSError **)error;
- (void)checkForUpdates;
@end

@protocol PatinaSparkleDriver <NSObject>
- (instancetype)initWithHostBundle:(NSBundle *)host delegate:(id)delegate;
- (void)dismissUpdateInstallation;
- (void)showUserInitiatedUpdateCheckWithCancellation:(void (^)(void))cancellation;
- (void)showDownloadInitiatedWithCancellation:(void (^)(void))cancellation;
- (void)showDownloadDidStartExtractingUpdate;
- (void)showUpdateFoundWithAppcastItem:(id)item state:(id)state reply:(void (^)(NSInteger))reply;
- (void)showReadyToInstallAndRelaunch:(void (^)(NSInteger))reply;
- (void)showInstallingUpdateWithApplicationTerminated:(BOOL)terminated retryTerminatingApplication:(void (^)(void))retry;
@end

// SPUUserUpdateChoice / SPUUserUpdateStage values from the pinned public headers.
static const NSInteger PatinaSkip = 0;
static const NSInteger PatinaInstalling = 2;

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
@property(copy) NSString *failureMessage;
@property uint64_t session;
@property BOOL versionAccepted;
@property uint64_t restartAttempt;
@property(copy) void (^restartHandler)(void);
@property(copy) void (^cancelPrepared)(void);
@end

@interface PatinaSparkleUserDriver : NSObject
@property(strong) id<PatinaSparkleDriver> standard;
@property(weak) PatinaSparkleDelegate *delegate;
@property(copy) void (^cancellation)(void);
@property BOOL extracting;
@property BOOL abandoned;
@property BOOL committed;
@end

@interface PatinaSparkleController : NSObject
@property(strong) id<PatinaSparkleUpdater> updater;
@property(strong) PatinaSparkleUserDriver *userDriver;
@end
@implementation PatinaSparkleController
@end

// Mailbox only: 0 absent, 1 starting, 2 cancelable, 3 restart requested,
// 4 finished, 5 failed, 6 restart authorized, 7 extraction/preparation,
// 8 prepared but quit deferred. Runtime/Ui pointers never cross threads.
static uint64_t PatinaSession;
static uint64_t PatinaNextSession;
static uint8_t PatinaStatus;
static PatinaSparkleController *PatinaUpdaterController;
static PatinaSparkleDelegate *PatinaUpdaterDelegate;

static BOOL PatinaMatches(uint64_t session) {
    @synchronized(PatinaSparkleDelegate.class) { return session != 0 && PatinaSession == session; }
}

static void PatinaPublish(uint64_t session, uint8_t status) {
    @synchronized(PatinaSparkleDelegate.class) {
        if (PatinaSession == session) PatinaStatus = status;
    }
}

static void PatinaCleanup(uint64_t session) {
    assert(NSThread.isMainThread);
    if (PatinaUpdaterDelegate.session != session) return;
    PatinaUpdaterDelegate.restartHandler = nil;
    PatinaUpdaterDelegate.cancelPrepared = nil;
    PatinaUpdaterController.userDriver.cancellation = nil;
    [PatinaUpdaterController.userDriver.standard dismissUpdateInstallation];
    // After Stage1, Sparkle deliberately permits its external installer to
    // survive updater disconnection and finish on a later normal app quit.
    PatinaUpdaterController = nil;
    PatinaUpdaterDelegate = nil;
}

static BOOL PatinaVersion(PatinaSparkleDelegate *delegate, id item) {
    return PatinaMatches(delegate.session) &&
        [[item valueForKey:@"displayVersionString"] isEqual:delegate.expectedVersion];
}

@implementation PatinaSparkleUserDriver
- (void)requestInstallWithReply:(void (^)(NSInteger))reply {
    if (self.abandoned || !PatinaMatches(self.delegate.session) || !self.delegate.versionAccepted) {
        uint64_t session = self.delegate.session;
        reply(PatinaSkip);
        dispatch_async(dispatch_get_main_queue(), ^{ PatinaCleanup(session); });
        return;
    }
    // Do not consume the one-shot public reply until the owning shell has
    // checked recordings. Disposal can still answer Skip instead of Install.
    self.delegate.restartHandler = ^{ reply(1); };
    self.delegate.cancelPrepared = ^{ reply(PatinaSkip); };
    PatinaPublish(self.delegate.session, 3);
}
// Preserve the pinned standard UI while observing public user-driver boundaries.
- (BOOL)respondsToSelector:(SEL)selector {
    return [super respondsToSelector:selector] || [self.standard respondsToSelector:selector];
}
- (BOOL)conformsToProtocol:(Protocol *)protocol {
    return [super conformsToProtocol:protocol] || [self.standard conformsToProtocol:protocol];
}
- (id)forwardingTargetForSelector:(SEL)selector {
    return [self.standard respondsToSelector:selector] ? self.standard : [super forwardingTargetForSelector:selector];
}
- (void)showUserInitiatedUpdateCheckWithCancellation:(void (^)(void))cancellation {
    self.cancellation = cancellation;
    __weak PatinaSparkleUserDriver *weak = self;
    [self.standard showUserInitiatedUpdateCheckWithCancellation:^{
        weak.cancellation = nil;
        cancellation();
    }];
}
- (void)showDownloadInitiatedWithCancellation:(void (^)(void))cancellation {
    self.cancellation = cancellation;
    __weak PatinaSparkleUserDriver *weak = self;
    [self.standard showDownloadInitiatedWithCancellation:^{
        weak.cancellation = nil;
        cancellation();
    }];
}
- (void)showDownloadDidStartExtractingUpdate {
    self.cancellation = nil;
    self.extracting = YES;
    PatinaPublish(self.delegate.session, 7);
    [self.standard showDownloadDidStartExtractingUpdate];
}
- (void)showUpdateFoundWithAppcastItem:(id)item state:(id)state reply:(void (^)(NSInteger))reply {
    self.cancellation = nil;
    self.delegate.versionAccepted = PatinaVersion(self.delegate, item);
    // This public boundary also covers resumed installers, for which Sparkle
    // intentionally does not call shouldProceedWithUpdate again.
    if (!self.delegate.versionAccepted || self.abandoned) {
        uint64_t session = self.delegate.session;
        reply(PatinaSkip);
        dispatch_async(dispatch_get_main_queue(), ^{ PatinaCleanup(session); PatinaPublish(session, 5); });
        return;
    }
    BOOL prepared = [[state valueForKey:@"stage"] integerValue] == PatinaInstalling;
    if (prepared) {
        self.extracting = YES;
        PatinaPublish(self.delegate.session, 7);
    }
    if (!prepared) PatinaPublish(self.delegate.session, 2);
    __weak PatinaSparkleUserDriver *weak = self;
    [self.standard showUpdateFoundWithAppcastItem:item state:state reply:^(NSInteger choice) {
        PatinaSparkleUserDriver *driver = weak;
        if (driver && choice == 1 && prepared) {
            [driver requestInstallWithReply:reply];
        } else {
            reply(driver && !driver.abandoned && PatinaMatches(driver.delegate.session) ? choice : PatinaSkip);
        }
    }];
}
- (void)showReadyToInstallAndRelaunch:(void (^)(NSInteger))reply {
    self.extracting = YES;
    self.cancellation = nil;
    if (self.abandoned || !PatinaMatches(self.delegate.session) || !self.delegate.versionAccepted) {
        uint64_t session = self.delegate.session;
        reply(PatinaSkip); // Public Skip really cancels the prepared installation.
        dispatch_async(dispatch_get_main_queue(), ^{ PatinaCleanup(session); });
        return;
    }
    PatinaPublish(self.delegate.session, 7);
    __weak PatinaSparkleUserDriver *weak = self;
    [self.standard showReadyToInstallAndRelaunch:^(NSInteger choice) {
        PatinaSparkleUserDriver *driver = weak;
        if (driver && choice == 1) {
            [driver requestInstallWithReply:reply];
        } else {
            reply(driver && !driver.abandoned && PatinaMatches(driver.delegate.session) ? choice : PatinaSkip);
        }
    }];
}
- (void)showInstallingUpdateWithApplicationTerminated:(BOOL)terminated retryTerminatingApplication:(void (^)(void))retry {
    __weak PatinaSparkleUserDriver *weak = self;
    [self.standard showInstallingUpdateWithApplicationTerminated:terminated retryTerminatingApplication:^{
        PatinaSparkleUserDriver *driver = weak;
        if (driver && !driver.abandoned && driver.delegate.versionAccepted && PatinaMatches(driver.delegate.session)) {
            // A retry must go through the shell's current recording guard too.
            driver.delegate.restartHandler = retry;
            PatinaPublish(driver.delegate.session, 3);
        }
    }];
}
@end

@implementation PatinaSparkleDelegate
- (BOOL)updater:(id)updater shouldProceedWithUpdate:(id)item updateCheck:(NSInteger)check error:(NSError **)error {
    self.versionAccepted = PatinaVersion(self, item);
    if (self.versionAccepted) return YES;
    if (error) *error = [NSError errorWithDomain:@"PatinaUpdate" code:1
        userInfo:@{NSLocalizedDescriptionKey: self.failureMessage}];
    return NO;
}
- (void)updater:(id)updater didFindValidUpdate:(id)item {
    self.versionAccepted = PatinaVersion(self, item);
    // A resumed item may already be prepared. Stay noncancelable until the
    // public user-driver state identifies whether this is a fresh download.
    PatinaPublish(self.session, 7);
}
- (void)updater:(id)updater willExtractUpdate:(id)item {
    if (!PatinaMatches(self.session) || PatinaUpdaterDelegate != self) return;
    PatinaUpdaterController.userDriver.cancellation = nil;
    PatinaUpdaterController.userDriver.extracting = YES;
    PatinaPublish(self.session, 7);
}
- (BOOL)updaterShouldRelaunchApplication:(id)updater {
    PatinaSparkleUserDriver *driver = PatinaUpdaterController.userDriver;
    return PatinaUpdaterDelegate == self && driver.committed && self.versionAccepted && !driver.abandoned;
}
- (void)updater:(id)updater didFinishUpdateCycleForUpdateCheck:(NSInteger)check error:(NSError *)error {
    if (PatinaUpdaterDelegate != self) return;
    if (PatinaUpdaterController.userDriver.abandoned && !PatinaUpdaterController.userDriver.committed) {
        PatinaCleanup(self.session);
    } else if (error) {
        uint64_t session = self.session;
        PatinaCleanup(session);
        PatinaPublish(session, 5);
    } else if (!self.restartHandler && !PatinaUpdaterController.userDriver.committed) {
        PatinaPublish(self.session, 4);
    }
}
@end

uint8_t patina_sparkle_status(uint64_t session) {
    @synchronized(PatinaSparkleDelegate.class) { return PatinaSession == session ? PatinaStatus : 0; }
}

bool patina_sparkle_can_cancel(void) {
    @synchronized(PatinaSparkleDelegate.class) {
        return PatinaSession == 0 || PatinaStatus == 1 || PatinaStatus == 2 || PatinaStatus == 4 || PatinaStatus == 5 || PatinaStatus == 8;
    }
}

static uint64_t PatinaStart(NSString *expected, NSString *failure) {
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
            Class updater = NSClassFromString(@"SPUUpdater");
            Class standard = NSClassFromString(@"SPUStandardUserDriver");
            if (!updater || !standard) { PatinaPublish(session, 5); return; }
            PatinaUpdaterDelegate = [PatinaSparkleDelegate new];
            PatinaUpdaterDelegate.session = session;
            PatinaUpdaterController = [PatinaSparkleController new];
            PatinaSparkleUserDriver *driver = [PatinaSparkleUserDriver new];
            driver.standard = [(id<PatinaSparkleDriver>)[standard alloc] initWithHostBundle:NSBundle.mainBundle delegate:nil];
            driver.delegate = PatinaUpdaterDelegate;
            PatinaUpdaterController.userDriver = driver;
            PatinaUpdaterController.updater = [(id<PatinaSparkleUpdater>)[updater alloc]
                initWithHostBundle:NSBundle.mainBundle applicationBundle:NSBundle.mainBundle userDriver:driver delegate:PatinaUpdaterDelegate];
            PatinaUpdaterController.updater.automaticallyChecksForUpdates = NO;
            PatinaUpdaterController.updater.automaticallyDownloadsUpdates = NO;
            if (![PatinaUpdaterController.updater startUpdater:NULL]) {
                PatinaCleanup(session);
                PatinaPublish(session, 5);
                return;
            }
        }
        PatinaUpdaterDelegate.session = session;
        PatinaUpdaterDelegate.expectedVersion = expected;
        PatinaUpdaterDelegate.failureMessage = failure;
        PatinaUpdaterDelegate.versionAccepted = NO;
        PatinaPublish(session, 2);
        [PatinaUpdaterController.updater checkForUpdates];
    });
    return session;
}

uint64_t patina_sparkle_start(const char *version, const char *failure) {
    if (!version || !failure || !patina_sparkle_supported()) return 0;
    NSString *expected = [NSString stringWithUTF8String:version];
    NSString *message = [NSString stringWithUTF8String:failure];
    return expected.length && message.length ? PatinaStart(expected, message) : 0;
}

static void PatinaDeferred(uint64_t session, uint64_t attempt) {
    uint8_t status = patina_sparkle_status(session);
    if (PatinaUpdaterDelegate.session != session || PatinaUpdaterDelegate.restartAttempt != attempt ||
        !PatinaUpdaterController.userDriver.committed || (status != 6 && status != 0)) return;
    PatinaCleanup(session);
    PatinaPublish(session, 8);
}

void patina_sparkle_allow_restart(uint64_t session) {
    @synchronized(PatinaSparkleDelegate.class) {
        if (PatinaSession != session || PatinaStatus != 3) return;
        PatinaStatus = 6;
    }
    dispatch_async(dispatch_get_main_queue(), ^{
        if (!PatinaMatches(session) || PatinaUpdaterDelegate.session != session) return;
        void (^install)(void) = PatinaUpdaterDelegate.restartHandler;
        void (^skip)(void) = PatinaUpdaterDelegate.cancelPrepared;
        PatinaUpdaterDelegate.restartHandler = nil;
        PatinaUpdaterDelegate.cancelPrepared = nil;
        if (!install || !PatinaUpdaterDelegate.versionAccepted) {
            if (skip) skip();
            dispatch_async(dispatch_get_main_queue(), ^{ PatinaCleanup(session); PatinaPublish(session, 5); });
            return;
        }
        PatinaUpdaterController.userDriver.committed = YES;
        uint64_t attempt = ++PatinaUpdaterDelegate.restartAttempt;
        install();
        // Sparkle's external agent observes process exit, not Cocoa's
        // WillTerminate notification. A declined/delayed quit must not leave
        // the recording host stopped forever. The prepared installation may
        // still apply on a later quit; this is recovery, not cancellation.
        dispatch_after(dispatch_time(DISPATCH_TIME_NOW, 60 * NSEC_PER_SEC), dispatch_get_main_queue(), ^{ PatinaDeferred(session, attempt); });
    });
}

bool patina_sparkle_try_cancel(void) {
    uint64_t session;
    @synchronized(PatinaSparkleDelegate.class) {
        if (!patina_sparkle_can_cancel()) return false;
        session = PatinaSession;
        uint8_t phase = PatinaStatus;
        PatinaSession = 0;
        if (session == 0) return true;
        dispatch_async(dispatch_get_main_queue(), ^{
            if (PatinaUpdaterDelegate.session != session) return;
            PatinaSparkleUserDriver *driver = PatinaUpdaterController.userDriver;
            if (driver.extracting && (phase == 1 || phase == 2)) {
                // Extraction may win the main-queue race after acceptance.
                // Keep the driver until Ready can answer genuine Skip.
                driver.abandoned = YES;
                [driver.standard dismissUpdateInstallation];
                return;
            }
            void (^cancel)(void) = driver.cancellation;
            driver.cancellation = nil;
            if (cancel) cancel();
            PatinaCleanup(session);
        });
    }
    return true;
}

void patina_sparkle_cancel(uint64_t session) {
    @synchronized(PatinaSparkleDelegate.class) {
        if (PatinaSession != session) return;
        if (patina_sparkle_can_cancel()) { patina_sparkle_try_cancel(); return; }
        PatinaSession = 0;
        dispatch_async(dispatch_get_main_queue(), ^{
            if (PatinaUpdaterDelegate.session != session) return;
            PatinaSparkleUserDriver *driver = PatinaUpdaterController.userDriver;
            if (driver.committed) return; // watchdog or process exit owns this handoff
            driver.abandoned = YES;
            PatinaUpdaterDelegate.restartHandler = nil;
            void (^skip)(void) = PatinaUpdaterDelegate.cancelPrepared;
            PatinaUpdaterDelegate.cancelPrepared = nil;
            if (skip) {
                skip();
                dispatch_async(dispatch_get_main_queue(), ^{ PatinaCleanup(session); });
            } else if (driver.extracting) {
                // Keep only native state until the public ready callback can
                // answer Skip, which cancels the prepared external installer.
                [driver.standard dismissUpdateInstallation];
            } else {
                PatinaCleanup(session);
            }
        });
    }
}
