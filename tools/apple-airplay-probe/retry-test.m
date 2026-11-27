// Exercise the probe's native retry controls offscreen. No route picker, window or fixture request.
#import "../../crates/iptv-app/src/host/native/apple_airplay.m"

@interface FailedPlayer : AVPlayer
@end
@implementation FailedPlayer
- (AVPlayerStatus)status { return AVPlayerStatusFailed; }
@end

@interface FailedItem : AVPlayerItem
@end
@implementation FailedItem
- (AVPlayerItemStatus)status { return AVPlayerItemStatusFailed; }
@end

@interface SyntheticRoutePlayer : AVPlayer
@property (nonatomic) BOOL syntheticExternal;
@end
@implementation SyntheticRoutePlayer
- (BOOL)isExternalPlaybackActive { return self.syntheticExternal; }
@end
static NSMutableArray<NSNumber *> *routeEvents;
static void capture(const void *context, bool external, uint8_t state) { [routeEvents addObject:@(state)]; }

int main(void) {
    @autoreleasepool {
        // The inherited initializer leaves the native event callback disconnected, so refresh
        // cannot open a source. Exercise the same player binding and commands used by NSH.
        PatinaAirPlayPlayer *probe = [[PatinaAirPlayPlayer alloc] initWithFrame:NSZeroRect];
        NSURL *a = [NSURL URLWithString:@"http://10.0.0.1:8089/ch0/index.m3u8"];
        NSURL *b = [NSURL URLWithString:@"http://10.0.0.1:8089/ch1/index.m3u8"];
        NSMutableSet *failed = [NSMutableSet setWithObjects:a, b, nil];
        [probe setValue:failed forKey:@"failedSources"];
        AVPlayer *player = [AVPlayer new];
        [probe setObservedPlayer:player];
        [probe setSource:a];
        [probe command:ProbeStop];
        [probe command:ProbePause];
        [probe setSource:b];
        [probe setSource:a];
        NSCAssert(![probe canLoadSource], @"passive updates must preserve the failed source");

        [probe command:ProbePlay];
        NSCAssert([probe canLoadSource] && [failed containsObject:b], @"Play retries only the selected source");
        NSCAssert([probe valueForKey:@"player"] == player, @"retain a healthy player and its route");

        [player replaceCurrentItemWithPlayerItem:[FailedItem playerItemWithAsset:[AVMutableComposition composition]]];
        [probe command:ProbePlay];
        NSCAssert(player.currentItem == nil, @"discard a failed item even before its KVO refresh");

        [probe setObservedPlayer:[FailedPlayer new]];
        NSCAssert(![probe canLoadSource], @"a terminally failed player cannot load another item");
        [probe command:ProbePlay];
        AVPlayer *replacement = [probe valueForKey:@"player"];
        NSCAssert([probe canLoadSource] && replacement.allowsExternalPlayback, @"bind a fresh AirPlay player");
        [probe setObservedPlayer:nil];

        PatinaAirPlayPlayer *host = [[PatinaAirPlayPlayer alloc] initWithContext:NULL emit:capture poll:NULL];
        [host setOwnerID:41];
        AVPlayer *persistent = [host routingPlayer];
        @autoreleasepool {
            PatinaCastButton *button = [[PatinaCastButton alloc] initWithFrame:NSZeroRect];
            button.ownerID = 41;
            NSCAssert(button.player == persistent, @"the device button shares the runtime player");
        }
        NSCAssert([host routingPlayer] == persistent, @"hiding the play bar must retain playback");
        PatinaCastButton *nextButton = [[PatinaCastButton alloc] initWithFrame:NSZeroRect];
        nextButton.ownerID = 41;
        [host disconnect];
        NSCAssert(nextButton.player == [host routingPlayer] && nextButton.player != persistent,
                  @"recreated controls follow the runtime's new player");
        [host invalidate];
        NSCAssert(nextButton.player == nil, @"destroyed owners cannot keep routing through a stale player");

        // Opening and dismissing the picker must not request a runtime handoff.
        // A receiver may approve after the sheet closes. Only an external
        // video KVO update on the primed player starts a handoff.
        routeEvents = [NSMutableArray array];
        PatinaAirPlayPlayer *choosing = [[PatinaAirPlayPlayer alloc] initWithContext:NULL emit:capture poll:NULL];
        [choosing setOwnerID:43];
        SyntheticRoutePlayer *standby = [SyntheticRoutePlayer new];
        [choosing setObservedPlayer:standby];
        [choosing routesWillPresent];
        NSCAssert(standby.currentItem && standby.rate == 0, @"negotiate with a paused local frame");
        NSURL *frameFile = [(AVURLAsset *)standby.currentItem.asset URL];
        NSCAssert(frameFile.isFileURL && [[NSFileManager defaultManager] fileExistsAtPath:frameFile.path],
                  @"the priming asset is local");
        NSDate *readyBy = [NSDate dateWithTimeIntervalSinceNow:5];
        while (standby.currentItem.status == AVPlayerItemStatusUnknown && readyBy.timeIntervalSinceNow > 0)
            [[NSRunLoop currentRunLoop] runUntilDate:[NSDate dateWithTimeIntervalSinceNow:0.02]];
        NSCAssert(standby.currentItem.status == AVPlayerItemStatusReadyToPlay &&
                  standby.currentItem.presentationSize.width == 16,
                  @"AVFoundation can actually decode the local video priming item");
        [choosing confirmPickerRoute:NO externalVideo:YES];
        [choosing confirmPickerRoute:YES externalVideo:NO];
        NSCAssert(![routeEvents containsObject:@8], @"existing routes and audio-only changes leave live video local");
        [choosing routesDidEnd];
        [choosing refresh];
        NSCAssert(![routeEvents containsObject:@8], @"dismissal must preserve the current playback owner");
        [choosing expireRouteChoice];
        standby.syntheticExternal = YES;
        [choosing refresh];
        NSCAssert(!standby.currentItem && ![routeEvents containsObject:@8], @"expired picker cannot change playback");
        NSCAssert([choosing routingPlayer] != standby, @"expiry releases the priming player's route");
        standby = [SyntheticRoutePlayer new];
        [choosing setObservedPlayer:standby];
        [choosing routesWillPresent];
        AVPlayerItem *primed = standby.currentItem;
        [choosing routesDidEnd];
        standby.syntheticExternal = YES;
        [choosing refresh];
        [choosing refresh];
        NSCAssert([routeEvents filteredArrayUsingPredicate:[NSPredicate predicateWithFormat:@"SELF == 8"]].count == 1,
                  @"late approval starts exactly one handoff");
        [choosing command:ProbeStop];
        NSCAssert(standby.currentItem == primed, @"Prepare preserves the routing item until replacement");
        uint64_t accepted = [choosing choiceSerial];
        [choosing finishPickerChoice:accepted-1];
        NSCAssert(standby.currentItem == primed, @"stale replies cannot release the newer choice");
        [choosing finishPickerChoice:accepted];
        NSCAssert(standby.currentItem == nil && [choosing routingPlayer] != standby,
                  @"a rejected handoff immediately releases its priming frame and route");
        [choosing invalidate];
        NSCAssert(![[NSFileManager defaultManager] fileExistsAtPath:frameFile.path], @"remove temporary frame");

        // An external-video KVO update can precede the audio/local-route
        // update. It must not destroy the owned item while selection is open.
        routeEvents = [NSMutableArray array];
        PatinaAirPlayPlayer *transfer = [[PatinaAirPlayPlayer alloc] initWithContext:NULL emit:capture poll:NULL];
        [transfer setOwnerID:42];
        SyntheticRoutePlayer *synthetic = [SyntheticRoutePlayer new];
        synthetic.syntheticExternal = YES;
        [transfer setObservedPlayer:synthetic];
        AVPlayerItem *item = [AVPlayerItem playerItemWithAsset:[AVMutableComposition composition]];
        [synthetic replaceCurrentItemWithPlayerItem:item];
        NSURL *opaque = [NSURL URLWithString:@"http://192.0.2.1/generated/not-fetched.m3u8"];
        [transfer setValue:opaque forKey:@"source"];
        [transfer setValue:opaque forKey:@"loaded"];
        [transfer setValue:@(ProbePlay) forKey:@"intent"];
        [transfer setValue:@YES forKey:@"playIssued"];
        [transfer setValue:@YES forKey:@"wasRouted"];
        [transfer routesWillPresent];
        [transfer finishPickerChoice:[transfer choiceSerial]];
        NSCAssert(synthetic.currentItem == item, @"a completed handoff retains its live media");
        synthetic.syntheticExternal = NO;
        [transfer refresh];
        NSCAssert(synthetic.currentItem == item, @"transient route selection must retain the current stream");
        NSCAssert(![routeEvents containsObject:@9], @"an intermediate KVO event is not a local destination choice");
        [transfer routesDidEnd];
        [transfer setValue:@([NSDate timeIntervalSinceReferenceDate] - 0.6) forKey:@"routeRecentlyClosed"];
        [transfer refresh];
        NSCAssert([routeEvents containsObject:@9] && synthetic.currentItem == nil,
                  @"settled local selection ends the external item and asks the runtime to resume locally");
        [transfer invalidate];
        routeEvents = nil;
    }
    return 0;
}
