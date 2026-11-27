// Synthetic public-API bridge contract; no server, installer or signing identity.
#import "../../../crates/iptv-app/src/host/updates/macos_sparkle.m"
#include <assert.h>

@interface SyntheticUpdateItem : NSObject
@property(copy) NSString *displayVersionString;
@property NSInteger stage;
@end
@implementation SyntheticUpdateItem
@end

static NSUInteger Aborted;
@interface SyntheticUpdater : NSObject <PatinaSparkleUpdater>
@property BOOL automaticallyChecksForUpdates;
@property BOOL automaticallyDownloadsUpdates;
@property BOOL sessionInProgress;
@property BOOL checked;
@end
@implementation SyntheticUpdater
- (instancetype)initWithHostBundle:(NSBundle *)host applicationBundle:(NSBundle *)application userDriver:(id)driver delegate:(id)delegate { return [super init]; }
- (BOOL)startUpdater:(NSError **)error { assert(NSThread.isMainThread); return YES; }
- (void)checkForUpdates { assert(NSThread.isMainThread); self.checked = YES; self.sessionInProgress = YES; }
- (void)dealloc { if (_sessionInProgress) Aborted++; }
@end

@interface SyntheticStandard : NSObject <PatinaSparkleDriver>
@property(copy) void (^cancellation)(void);
@property(copy) void (^choice)(NSInteger);
@property(copy) void (^retry)(void);
@property BOOL dismissed;
@end
@implementation SyntheticStandard
- (instancetype)initWithHostBundle:(NSBundle *)host delegate:(id)delegate { return [super init]; }
- (void)dismissUpdateInstallation { self.dismissed = YES; self.cancellation = nil; self.choice = nil; self.retry = nil; }
- (void)showUserInitiatedUpdateCheckWithCancellation:(void (^)(void))cancel { self.cancellation = cancel; }
- (void)showDownloadInitiatedWithCancellation:(void (^)(void))cancel { self.cancellation = cancel; }
- (void)showDownloadDidStartExtractingUpdate { self.cancellation = nil; }
- (void)showUpdateFoundWithAppcastItem:(id)item state:(id)state reply:(void (^)(NSInteger))reply { self.choice = reply; }
- (void)showReadyToInstallAndRelaunch:(void (^)(NSInteger))reply { self.choice = reply; }
- (void)showInstallingUpdateWithApplicationTerminated:(BOOL)terminated retryTerminatingApplication:(void (^)(void))retry { self.retry = retry; }
@end

static void background(void (^work)(void)) {
    dispatch_semaphore_t done = dispatch_semaphore_create(0);
    [NSThread detachNewThreadWithBlock:^{ @autoreleasepool {
        assert(!NSThread.isMainThread); work(); dispatch_semaphore_signal(done);
    }}];
    assert(dispatch_semaphore_wait(done, dispatch_time(DISPATCH_TIME_NOW, 5 * NSEC_PER_SEC)) == 0);
}
static void drainMain(void) {
    __block BOOL finished = NO;
    dispatch_async(dispatch_get_main_queue(), ^{ finished = YES; });
    while (!finished) CFRunLoopRunInMode(kCFRunLoopDefaultMode, 0.01, true);
}
static uint64_t synthetic(void) {
    assert(PatinaSession == 0);
    PatinaUpdaterDelegate = [PatinaSparkleDelegate new];
    PatinaUpdaterController = [PatinaSparkleController new];
    PatinaUpdaterController.updater = [SyntheticUpdater new];
    PatinaUpdaterController.userDriver = [PatinaSparkleUserDriver new];
    PatinaUpdaterController.userDriver.standard = [SyntheticStandard new];
    PatinaUpdaterController.userDriver.delegate = PatinaUpdaterDelegate;
    __block uint64_t session;
    background(^{
        session = PatinaStart(@"0.2.0", @"Synthetic localized failure");
        assert(session != 0); assert(patina_sparkle_status(session) == 1);
        assert(PatinaStart(@"0.3.0", @"failure") == 0);
    });
    drainMain();
    assert(patina_sparkle_status(session) == 2);
    return session;
}

// AppKit really can cancel Cocoa termination and still return normally from
// its event loop, as the pinned NSH runner does. Sparkle observes process exit.
@interface SyntheticQuitDelegate : NSObject <NSApplicationDelegate>
@property(copy) NSString *marker;
@end
@implementation SyntheticQuitDelegate
- (NSApplicationTerminateReply)applicationShouldTerminate:(NSApplication *)app {
    [@"Cocoa cancelled; run loop stopped" writeToFile:self.marker atomically:YES encoding:NSUTF8StringEncoding error:NULL];
    [app stop:nil];
    NSEvent *wake = [NSEvent otherEventWithType:NSEventTypeApplicationDefined location:NSZeroPoint modifierFlags:0 timestamp:0 windowNumber:0 context:nil subtype:0 data1:0 data2:0];
    [app postEvent:wake atStart:NO];
    return NSTerminateCancel;
}
- (void)applicationWillTerminate:(NSNotification *)notification { abort(); }
@end

int main(int argc, const char **argv) {
    @autoreleasepool {
        if (argc == 3 && strcmp(argv[1], "--quit-child") == 0) {
            NSApplication *app = NSApplication.sharedApplication;
            [app setActivationPolicy:NSApplicationActivationPolicyProhibited];
            SyntheticQuitDelegate *delegate = [SyntheticQuitDelegate new];
            delegate.marker = [NSString stringWithUTF8String:argv[2]];
            app.delegate = delegate;
            dispatch_async(dispatch_get_main_queue(), ^{ [app terminate:nil]; });
            [app run];
            assert([[NSString stringWithContentsOfFile:delegate.marker encoding:NSUTF8StringEncoding error:NULL] containsString:@"Cocoa cancelled"]);
            app.delegate = nil;
            return 0; // Parent observes normal process exit without WillTerminate.
        }
        NSMutableDictionary *info = [@{@"PatinaSparkleEnabled":@YES,@"SUEnableAutomaticChecks":@NO,@"SUAllowsAutomaticUpdates":@NO,@"SUFeedURL":@"https://updates.example.test/appcast.xml",@"SUPublicEDKey":@"AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA="} mutableCopy];
        assert(PatinaSparkleConfiguration(info));
        for (NSString *field in info.allKeys) {
            NSMutableDictionary *partial = [info mutableCopy]; [partial removeObjectForKey:field];
            assert(!PatinaSparkleConfiguration(partial));
        }
        for (NSString *feed in @[@"http://updates.example.test/a",@"https://user@updates.example.test/a",@"https://updates.example.test/a#fragment"]) {
            NSMutableDictionary *invalid = [info mutableCopy]; invalid[@"SUFeedURL"] = feed;
            assert(!PatinaSparkleConfiguration(invalid));
        }
        background(^{ assert(!patina_sparkle_supported()); assert(patina_sparkle_start("0.2.0","failure") == 0); });
        assert(!PatinaDeveloperIDProcess());
        __block NSUInteger cancellations = 0, installs = 0;
        uint64_t session = synthetic();
        PatinaSparkleUserDriver *driver = PatinaUpdaterController.userDriver;
        [driver showDownloadInitiatedWithCancellation:^{ cancellations++; }];
        background(^{ assert(patina_sparkle_can_cancel()); assert(patina_sparkle_try_cancel()); });
        drainMain();
        assert(cancellations == 1 && PatinaUpdaterController == nil);
        assert(patina_sparkle_status(session) == 0);

        session = synthetic(); driver = PatinaUpdaterController.userDriver;
        [driver showDownloadInitiatedWithCancellation:^{ cancellations++; }];
        [PatinaUpdaterDelegate updater:nil willExtractUpdate:nil];
        [driver showDownloadDidStartExtractingUpdate];
        assert(patina_sparkle_status(session) == 7);
        background(^{ assert(!patina_sparkle_can_cancel()); assert(!patina_sparkle_try_cancel()); });
        patina_sparkle_cancel(session); drainMain();
        assert(driver.abandoned && PatinaUpdaterController != nil);
        __block NSInteger abandonedChoice = -1;
        [driver showReadyToInstallAndRelaunch:^(NSInteger choice) { abandonedChoice = choice; }];
        drainMain();
        assert(abandonedChoice == PatinaSkip && PatinaUpdaterController == nil);
        assert(cancellations == 1);

        session = synthetic(); driver = PatinaUpdaterController.userDriver;
        SyntheticUpdateItem *item = [SyntheticUpdateItem new]; item.displayVersionString = @"0.3.0"; item.stage = PatinaInstalling;
        NSError *error = nil;
        assert(![PatinaUpdaterDelegate updater:nil shouldProceedWithUpdate:item updateCheck:0 error:&error]);
        assert([error.localizedDescription isEqual:@"Synthetic localized failure"]);
        __block NSInteger resumedChoice = -1;
        [driver showUpdateFoundWithAppcastItem:item state:item reply:^(NSInteger choice) { resumedChoice = choice; }];
        drainMain();
        assert(resumedChoice == PatinaSkip && patina_sparkle_status(session) == 5);
        patina_sparkle_cancel(session); drainMain();

        // A resumed Installing choice is held until the shell approves.
        session = synthetic(); driver = PatinaUpdaterController.userDriver;
        item.displayVersionString = @"0.2.0";
        [PatinaUpdaterDelegate updater:nil didFindValidUpdate:item];
        __block NSUInteger skipped = 0;
        [driver showUpdateFoundWithAppcastItem:item state:item reply:^(NSInteger choice) {
            if (choice == 1) installs++; else if (choice == PatinaSkip) skipped++;
        }];
        SyntheticStandard *standard = (SyntheticStandard *)driver.standard;
        standard.choice(1);
        assert(installs == 0 && !patina_sparkle_can_cancel());
        assert(![PatinaUpdaterDelegate updaterShouldRelaunchApplication:nil]);
        background(^{ patina_sparkle_allow_restart(session); patina_sparkle_cancel(session); });
        drainMain(); drainMain();
        assert(installs == 0 && skipped == 1 && PatinaUpdaterController == nil);

        // A Cancel accepted before the main-queue extraction callback must
        // retain the abandoned driver until Ready can send genuine Skip.
        session = synthetic(); driver = PatinaUpdaterController.userDriver;
        [driver showDownloadInitiatedWithCancellation:^{ cancellations++; }];
        background(^{ assert(patina_sparkle_try_cancel()); });
        [driver showDownloadDidStartExtractingUpdate];
        drainMain();
        assert(driver.abandoned && PatinaUpdaterController != nil);
        [driver showReadyToInstallAndRelaunch:^(NSInteger choice) { assert(choice == PatinaSkip); skipped++; }];
        drainMain();
        assert(skipped == 2 && PatinaUpdaterController == nil);

        session = synthetic(); driver = PatinaUpdaterController.userDriver;
        [PatinaUpdaterDelegate updater:nil didFindValidUpdate:item];
        [driver showReadyToInstallAndRelaunch:^(NSInteger choice) { if (choice == 1) installs++; }];
        standard = (SyntheticStandard *)driver.standard;
        standard.choice(1);
        assert(installs == 0 && patina_sparkle_status(session) == 3);
        background(^{ patina_sparkle_allow_restart(session); }); drainMain();
        assert(installs == 1 && patina_sparkle_status(session) == 6);
        assert([PatinaUpdaterDelegate updaterShouldRelaunchApplication:nil]);
        [PatinaUpdaterDelegate updater:nil didFinishUpdateCycleForUpdateCheck:0 error:nil];
        assert(patina_sparkle_status(session) == 6);
        [driver showInstallingUpdateWithApplicationTerminated:NO retryTerminatingApplication:^{ installs++; }];
        standard.retry();
        assert(patina_sparkle_status(session) == 3);
        PatinaDeferred(session, 1);
        assert(PatinaUpdaterController != nil);
        patina_sparkle_allow_restart(session); drainMain();
        assert(installs == 2 && PatinaUpdaterDelegate.restartAttempt == 2);
        PatinaDeferred(session, 1); assert(patina_sparkle_status(session) == 6);
        PatinaDeferred(session, 2);
        assert(patina_sparkle_status(session) == 8 && PatinaUpdaterController == nil);
        assert(patina_sparkle_can_cancel());
        patina_sparkle_cancel(session); drainMain();
        assert(PatinaSession == 0 && Aborted >= 6);

        // Version approval can be revoked after Ready, before the main queue
        // consumes Install. The pending reply must receive Skip exactly once.
        session = synthetic(); driver = PatinaUpdaterController.userDriver;
        [PatinaUpdaterDelegate updater:nil didFindValidUpdate:item];
        [driver showReadyToInstallAndRelaunch:^(NSInteger choice) { assert(choice == PatinaSkip); skipped++; }];
        standard = (SyntheticStandard *)driver.standard;
        standard.choice(1);
        PatinaUpdaterDelegate.versionAccepted = NO;
        patina_sparkle_allow_restart(session); drainMain(); drainMain();
        assert(skipped == 3 && patina_sparkle_status(session) == 5 && PatinaUpdaterController == nil);
        patina_sparkle_cancel(session); drainMain();

        // An actual post-approval installer error is failure, not a later
        // watchdog notice claiming that the package is still prepared.
        session = synthetic(); driver = PatinaUpdaterController.userDriver;
        [PatinaUpdaterDelegate updater:nil didFindValidUpdate:item];
        [driver showReadyToInstallAndRelaunch:^(NSInteger choice) { assert(choice == 1); }];
        standard = (SyntheticStandard *)driver.standard;
        standard.choice(1); patina_sparkle_allow_restart(session); drainMain();
        [PatinaUpdaterDelegate updater:nil didFinishUpdateCycleForUpdateCheck:0 error:[NSError errorWithDomain:@"Synthetic" code:1 userInfo:nil]];
        assert(patina_sparkle_status(session) == 5 && PatinaUpdaterController == nil);
        PatinaDeferred(session, 1); assert(patina_sparkle_status(session) == 5);
        patina_sparkle_cancel(session); drainMain();

        // Terminal native Skip/Dismiss must not retain an extraction driver
        // waiting for a second Ready callback that will never arrive.
        for (NSInteger choice = 0; choice <= 2; choice += 2) {
            session = synthetic(); driver = PatinaUpdaterController.userDriver;
            [PatinaUpdaterDelegate updater:nil didFindValidUpdate:item];
            __block NSInteger answer = -1;
            [driver showReadyToInstallAndRelaunch:^(NSInteger result) { answer = result; }];
            standard = (SyntheticStandard *)driver.standard;
            standard.choice(choice);
            assert(answer == choice);
            [PatinaUpdaterDelegate updater:nil didFinishUpdateCycleForUpdateCheck:0 error:nil];
            assert(patina_sparkle_status(session) == 4);
            patina_sparkle_cancel(session); drainMain();
            assert(PatinaSession == 0 && PatinaUpdaterController == nil);
        }
        session = synthetic(); driver = PatinaUpdaterController.userDriver;
        PatinaSparkleDelegate *old = PatinaUpdaterDelegate;
        [old updater:nil willExtractUpdate:item];
        patina_sparkle_cancel(session); drainMain();
        [old updater:nil didFinishUpdateCycleForUpdateCheck:0 error:[NSError errorWithDomain:@"Synthetic" code:2 userInfo:nil]];
        assert(PatinaUpdaterController == nil);
        session = synthetic();
        [old updater:nil willExtractUpdate:item];
        [old updater:nil didFindValidUpdate:item];
        [old updater:nil didFinishUpdateCycleForUpdateCheck:0 error:nil];
        assert(patina_sparkle_status(session) == 2 && !PatinaUpdaterController.userDriver.extracting);
        patina_sparkle_cancel(session); drainMain();

        // Root teardown after the approved one-shot Install must preserve the
        // external handoff. The orphaned watchdog can still release native UI.
        session = synthetic(); driver = PatinaUpdaterController.userDriver;
        [PatinaUpdaterDelegate updater:nil didFindValidUpdate:item];
        [driver showReadyToInstallAndRelaunch:^(NSInteger choice) { assert(choice == 1); }];
        standard = (SyntheticStandard *)driver.standard;
        standard.choice(1); patina_sparkle_allow_restart(session); drainMain();
        PatinaSparkleDelegate *committed = PatinaUpdaterDelegate;
        patina_sparkle_cancel(session); drainMain();
        assert(PatinaSession == 0 && PatinaUpdaterController != nil);
        assert([committed updaterShouldRelaunchApplication:nil]);
        PatinaDeferred(session, 1);
        assert(PatinaUpdaterController == nil);
    }
}
