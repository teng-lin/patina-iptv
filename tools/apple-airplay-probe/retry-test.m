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

int main(void) {
    @autoreleasepool {
        // The inherited initializer leaves the native event callback disconnected, so refresh
        // cannot open a source. Exercise the same player binding and commands used by NSH.
        PatinaAirPlayProbe *probe = [[PatinaAirPlayProbe alloc] initWithFrame:NSZeroRect];
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
    }
    return 0;
}
