// Synthetic bridge contract: no server, installer or real signing identity.
#import "../../../crates/iptv-app/src/host/updates/macos_sparkle.m"
#include <assert.h>

@interface SyntheticUpdateItem : NSObject
@property(copy) NSString *displayVersionString;
@end
@implementation SyntheticUpdateItem
@end

@interface SyntheticUpdater : NSObject <PatinaSparkleUpdater>
@property BOOL automaticallyChecksForUpdates;
@property BOOL automaticallyDownloadsUpdates;
@property BOOL sessionInProgress;
@property BOOL checked;
@end
static NSUInteger Aborted;
@implementation SyntheticUpdater
- (BOOL)startUpdater:(NSError **)error { assert(NSThread.isMainThread); return YES; }
- (void)checkForUpdates { assert(NSThread.isMainThread); self.checked = YES; self.sessionInProgress = YES; }
- (void)dealloc { if (_sessionInProgress) Aborted++; }
@end

@interface SyntheticController : NSObject <PatinaSparkleController>
@property(strong) id<PatinaSparkleUpdater> updater;
@property(readonly) id userDriver;
@end
@implementation SyntheticController
- (instancetype)initWithStartingUpdater:(BOOL)start updaterDelegate:(id)delegate userDriverDelegate:(id)driver { return [super init]; }
- (id)userDriver { return nil; }
@end

static void background(void (^work)(void)) {
    dispatch_semaphore_t done = dispatch_semaphore_create(0);
    [NSThread detachNewThreadWithBlock:^{
        @autoreleasepool {
            assert(!NSThread.isMainThread);
            work();
            dispatch_semaphore_signal(done);
        }
    }];
    assert(dispatch_semaphore_wait(done, dispatch_time(DISPATCH_TIME_NOW, 5 * NSEC_PER_SEC)) == 0);
}

static void drainMain(void) {
    __block BOOL finished = NO;
    dispatch_async(dispatch_get_main_queue(), ^{ finished = YES; });
    while (!finished) CFRunLoopRunInMode(kCFRunLoopDefaultMode, 0.01, true);
}

int main(void) {
    @autoreleasepool {
        NSMutableDictionary *info = [@{
            @"PatinaSparkleEnabled": @YES,
            @"SUEnableAutomaticChecks": @NO,
            @"SUAllowsAutomaticUpdates": @NO,
            @"SUFeedURL": @"https://updates.example.test/appcast.xml",
            @"SUPublicEDKey": @"AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA="
        } mutableCopy];
        assert(PatinaSparkleConfiguration(info));
        for (NSString *field in info.allKeys) {
            NSMutableDictionary *partial = [info mutableCopy];
            [partial removeObjectForKey:field];
            assert(!PatinaSparkleConfiguration(partial));
        }
        for (NSString *feed in @[@"http://updates.example.test/a", @"https://user@updates.example.test/a", @"https://updates.example.test/a#fragment"]) {
            NSMutableDictionary *invalid = [info mutableCopy];
            invalid[@"SUFeedURL"] = feed;
            assert(!PatinaSparkleConfiguration(invalid));
        }
        assert(!patina_sparkle_supported());
        assert(!PatinaDeveloperIDProcess());
        background(^{
            assert(!NSThread.isMainThread);
            assert(!patina_sparkle_supported());
            assert(patina_sparkle_start("0.2.0", "Synthetic localized failure") == 0);
        });
        __block uint64_t session;
        __block NSUInteger installs = 0;
        __weak SyntheticUpdater *updater;
        @autoreleasepool {
            PatinaSparkleDelegate *delegate = [PatinaSparkleDelegate new];
            PatinaUpdaterDelegate = delegate;
            PatinaUpdaterController = [SyntheticController new];
            ((SyntheticController *)PatinaUpdaterController).updater = [SyntheticUpdater new];
            updater = (SyntheticUpdater *)PatinaUpdaterController.updater;
            background(^{
                session = PatinaStart(@"0.2.0", @"Synthetic localized failure");
                assert(session != 0);
                assert(patina_sparkle_status(session) == 1);
                assert(PatinaStart(@"0.3.0", @"Synthetic localized failure") == 0);
            });
            assert(!updater.checked);
            drainMain();
            assert(updater.checked);
            assert(patina_sparkle_status(session) == 2);
            SyntheticUpdateItem *item = [SyntheticUpdateItem new];
            NSError *error = nil;
            item.displayVersionString = @"0.3.0";
            assert(![delegate updater:nil shouldProceedWithUpdate:item updateCheck:0 error:&error]);
            assert(error != nil);
            assert([error.localizedDescription isEqual:@"Synthetic localized failure"]);
            item.displayVersionString = @"0.2.0";
            assert([delegate updater:nil shouldProceedWithUpdate:item updateCheck:0 error:NULL]);
            assert([delegate updater:nil shouldPostponeRelaunchForUpdate:item untilInvokingBlock:^{ installs++; }]);
            assert(patina_sparkle_status(session) == 3);
            assert(installs == 0);
            background(^{
                patina_sparkle_allow_restart(session);
                assert(patina_sparkle_status(session) == 6);
            });
            assert(installs == 0);
            drainMain();
            assert(installs == 1);
            assert(delegate.restartHandler == nil);
            // Root disposal wins even after approval was queued to AppKit main.
            assert([delegate updater:nil shouldPostponeRelaunchForUpdate:item untilInvokingBlock:^{ installs++; }]);
            background(^{
                patina_sparkle_allow_restart(session);
                patina_sparkle_cancel(session);
                assert(patina_sparkle_status(session) == 0);
            });
            drainMain();
            assert(installs == 1);
            assert(delegate.restartHandler == nil);
            assert(PatinaUpdaterController == nil);
            assert(PatinaUpdaterDelegate == nil);
        }
        assert(updater == nil);
        assert(Aborted == 1);
        // A stale callback/approval cannot modify or authorize a new root.
        PatinaUpdaterController = [SyntheticController new];
        ((SyntheticController *)PatinaUpdaterController).updater = [SyntheticUpdater new];
        PatinaUpdaterDelegate = [PatinaSparkleDelegate new];
        __block uint64_t replacement;
        background(^{
            replacement = PatinaStart(@"0.3.0", @"Synthetic localized failure");
            PatinaPublish(session, 5);
            patina_sparkle_allow_restart(session);
        });
        drainMain();
        assert(replacement != session);
        assert(patina_sparkle_status(replacement) == 2);
        assert(installs == 1);
        patina_sparkle_cancel(replacement);
        drainMain();
    }
}
