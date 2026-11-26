// Native view for the opt-in AirPlay probe. NSH creates, updates and destroys it on main.
#import <AVFoundation/AVFoundation.h>
#import <AVKit/AVKit.h>
#import <TargetConditionals.h>
#include <stdbool.h>
#include <stdint.h>

#if TARGET_OS_OSX
#import <AppKit/AppKit.h>
#define PlatformView NSView
#else
#import <UIKit/UIKit.h>
#define PlatformView UIView
#endif

typedef NS_ENUM(uint8_t, ProbeState) {
    ProbeIdle, ProbeWaiting, ProbePlaying, ProbePaused, ProbeFailed, ProbeRouteLost
};
typedef NS_ENUM(uint8_t, ProbeCommand) { ProbePlay, ProbePause, ProbeStop };
typedef void (*StateCallback)(const void *, bool, uint8_t);

static void *PlayerObservation = &PlayerObservation;

@interface PatinaAirPlayProbe : PlatformView
- (instancetype)initWithContext:(const void *)context emit:(StateCallback)emit;
- (void)setSource:(NSURL *)source;
- (void)command:(ProbeCommand)command;
- (void)scheduleRefresh;
- (void)invalidate;
@end

@implementation PatinaAirPlayProbe {
    AVPlayer *_player;
    AVRoutePickerView *_picker;
    NSURL *_source;
    NSURL *_loaded;
    NSMutableSet<NSURL *> *_failedSources;
    const void *_context;
    StateCallback _emit;
    ProbeCommand _intent;
    ProbeState _stoppedState;
    ProbeState _lastState;
    BOOL _published;
    BOOL _lastExternal;
    BOOL _wasRouted;
    BOOL _playIssued;
#if !TARGET_OS_OSX
    BOOL _audioSessionActive;
#endif
}

- (instancetype)initWithContext:(const void *)context emit:(StateCallback)emit {
    self = [super initWithFrame:CGRectZero];
    if (!self) return nil;
    _context = context;
    _emit = emit;
    _intent = ProbeStop;
    _failedSources = [NSMutableSet set];
    _picker = [[AVRoutePickerView alloc] initWithFrame:self.bounds];
#if TARGET_OS_OSX
    _picker.autoresizingMask = NSViewWidthSizable | NSViewHeightSizable;
#else
    _picker.prioritizesVideoDevices = YES;
    _picker.autoresizingMask = UIViewAutoresizingFlexibleWidth | UIViewAutoresizingFlexibleHeight;
    [[NSNotificationCenter defaultCenter] addObserver:self selector:@selector(routeChanged:)
        name:AVAudioSessionRouteChangeNotification object:[AVAudioSession sharedInstance]];
    [[NSNotificationCenter defaultCenter] addObserver:self selector:@selector(audioInterrupted:)
        name:AVAudioSessionInterruptionNotification object:[AVAudioSession sharedInstance]];
#endif
    [self addSubview:_picker];
    [self setObservedPlayer:[AVPlayer new]];
    return self;
}

// Keep the picker and KVO attached to the same player, including after a terminal failure.
- (void)setObservedPlayer:(AVPlayer *)player {
    NSArray<NSString *> *keys = @[@"externalPlaybackActive", @"timeControlStatus", @"currentItem.status", @"status"];
    for (NSString *key in keys) [_player removeObserver:self forKeyPath:key context:PlayerObservation];
    [self clearItem];
    _wasRouted = NO;
    _player = player;
    _player.allowsExternalPlayback = YES;
#if TARGET_OS_OSX
    _picker.player = player;
#endif
    for (NSString *key in keys) [_player addObserver:self forKeyPath:key options:0 context:PlayerObservation];
}

- (void)publish:(ProbeState)state external:(BOOL)external {
    if (!_emit || (_published && _lastState == state && _lastExternal == external)) return;
    _published = YES;
    _lastState = state;
    _lastExternal = external;
    _emit(_context, external, state);
}

- (void)clearItem {
    _playIssued = NO;
    _loaded = nil;
    [_player pause];
    [_player replaceCurrentItemWithPlayerItem:nil];
}

#if !TARGET_OS_OSX
- (void)releaseAudioSession {
    if (!_audioSessionActive) return;
    _audioSessionActive = NO;
    [[AVAudioSession sharedInstance] setActive:NO
        withOptions:AVAudioSessionSetActiveOptionNotifyOthersOnDeactivation error:NULL];
}
#endif

- (void)stopWithState:(ProbeState)state {
    _intent = ProbeStop;
    _stoppedState = state;
    [self clearItem];
#if !TARGET_OS_OSX
    [self releaseAudioSession];
#endif
}

- (BOOL)canLoadSource {
    return _source && _player.status != AVPlayerStatusFailed && ![_failedSources containsObject:_source];
}

- (void)refresh {
    if (!_emit) return;
    BOOL external = _player.externalPlaybackActive;
    BOOL routed = external;
#if !TARGET_OS_OSX
    routed = NO;
    for (AVAudioSessionPortDescription *port in [AVAudioSession sharedInstance].currentRoute.outputs) {
        if ([port.portType isEqualToString:AVAudioSessionPortAirPlay]) {
            routed = YES;
            break;
        }
    }
    external = external && routed;
#endif
    if ((_wasRouted && !routed) || (_playIssued && !external)) {
        [_failedSources removeAllObjects];
        if (_intent != ProbeStop) [self stopWithState:ProbeRouteLost];
    }
    _wasRouted = routed;
    if (_intent == ProbeStop) {
        [self publish:_stoppedState external:external];
        return;
    }
    if (![self canLoadSource]) {
        [self stopWithState:ProbeFailed];
        [self publish:ProbeFailed external:external];
        return;
    }
#if !TARGET_OS_OSX
    // An iPad's output selection belongs to the audio session. A local route
    // has no reason to open the fixture or prepare an AVPlayerItem.
    if (!routed) {
        [self publish:ProbeWaiting external:NO];
        return;
    }
#endif
    if (![_loaded isEqual:_source]) {
        // On macOS the picker routes this specific AVPlayer. Prime its item
        // while paused so AVKit can negotiate video playback with the receiver.
        _loaded = _source;
        _playIssued = NO;
        [_player pause];
        [_player replaceCurrentItemWithPlayerItem:[AVPlayerItem playerItemWithURL:_source]];
        // Replacement can renegotiate or drop the external route. Read the
        // player's new route on the next main-queue turn before issuing play.
#if TARGET_OS_OSX
        _wasRouted = NO;
#endif
        [self scheduleRefresh];
        [self publish:ProbeWaiting external:_player.externalPlaybackActive];
        return;
    }
    if (_player.currentItem.status == AVPlayerItemStatusFailed) {
        [_failedSources addObject:_source];
        [self stopWithState:ProbeFailed];
        [self publish:ProbeFailed external:external];
        return;
    }
    if (_intent == ProbePause) {
        [self publish:ProbePaused external:external];
    } else if (!external) {
        [self publish:ProbeWaiting external:NO];
    } else {
        if (!_playIssued) {
            _playIssued = YES;
            [_player play];
        }
        [self publish:(_player.timeControlStatus == AVPlayerTimeControlStatusPlaying ?
                       ProbePlaying : ProbeWaiting) external:YES];
    }
}

- (void)scheduleRefresh {
    // AVFoundation may deliver KVO and route notifications off main. Read the
    // current item/route on main instead of applying stale notification values.
    __weak PatinaAirPlayProbe *weakSelf = self;
    dispatch_async(dispatch_get_main_queue(), ^{ [weakSelf refresh]; });
}

- (void)observeValueForKeyPath:(NSString *)key ofObject:(id)object
                        change:(NSDictionary *)change context:(void *)context {
    if (context == PlayerObservation) {
        [self scheduleRefresh];
    } else {
        [super observeValueForKeyPath:key ofObject:object change:change context:context];
    }
}

#if !TARGET_OS_OSX
- (void)routeChanged:(NSNotification *)notification {
    [self scheduleRefresh];
}

- (void)audioInterrupted:(NSNotification *)notification {
    if (!_audioSessionActive ||
        [notification.userInfo[AVAudioSessionInterruptionTypeKey] unsignedIntegerValue] !=
            AVAudioSessionInterruptionTypeBegan) return;
    // AVAudioSession delivers this on main after the system deactivates it.
    // Leave playback paused until another explicit Play acquires a new session.
    _audioSessionActive = NO;
    [self stopWithState:ProbePaused];
    [self refresh];
}
#endif

- (void)setSource:(NSURL *)source {
    if ([_source isEqual:source]) return;
    _source = source;
    if (!source) [self stopWithState:ProbeIdle];
    [self refresh];
}

- (void)command:(ProbeCommand)command {
    // Only a new user Play retries a failed fixture, e.g. after Local Network permission changed.
    if (command == ProbePlay && _source) {
        [_failedSources removeObject:_source];
        if (_player.status == AVPlayerStatusFailed) {
            // A failed AVPlayer cannot be reused for another item.
            [self setObservedPlayer:[AVPlayer new]];
        } else if (_player.currentItem.status == AVPlayerItemStatusFailed) {
            // A failure can precede its queued KVO refresh.
            [self clearItem];
        }
    }
#if !TARGET_OS_OSX
    if (command == ProbePlay && !_audioSessionActive && [self canLoadSource]) {
        AVAudioSession *audio = [AVAudioSession sharedInstance];
        if (![audio setCategory:AVAudioSessionCategoryPlayback mode:AVAudioSessionModeMoviePlayback
                       options:0 error:NULL] || ![audio setActive:YES error:NULL]) {
            [self stopWithState:ProbeFailed];
            [self publish:ProbeFailed external:NO];
            return;
        }
        _audioSessionActive = YES;
    }
#endif
    switch (command) {
        case ProbePlay: _intent = ProbePlay; break;
        case ProbePause:
            if (_intent != ProbeStop) _intent = ProbePause;
            _playIssued = NO;
            [_player pause];
            break;
        case ProbeStop: [self stopWithState:ProbeIdle]; break;
    }
    [self refresh];
}

- (void)invalidate {
    if (!_emit) return;
    _emit = NULL;
    [self setObservedPlayer:nil];
#if !TARGET_OS_OSX
    [[NSNotificationCenter defaultCenter] removeObserver:self];
    [self releaseAudioSession];
#endif
}
@end

void *patina_airplay_create(const void *context, StateCallback emit) {
    return (__bridge_retained void *)[[PatinaAirPlayProbe alloc] initWithContext:context emit:emit];
}

void patina_airplay_source(const void *view, const char *source) {
    NSURL *url = source ? [NSURL URLWithString:[NSString stringWithUTF8String:source]] : nil;
    if (![url.scheme isEqualToString:@"http"] || !url.host.length || url.user || url.password) url = nil;
    [(__bridge PatinaAirPlayProbe *)view setSource:url];
}

void patina_airplay_command(const void *view, uint8_t command) {
    [(__bridge PatinaAirPlayProbe *)view command:(ProbeCommand)command];
}

void patina_airplay_invalidate(const void *view) {
    [(__bridge PatinaAirPlayProbe *)view invalidate];
}
