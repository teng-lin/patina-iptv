// Native AirPlay player. NSH creates, updates and destroys the view on main.
#import <AVFoundation/AVFoundation.h>
#import <AVKit/AVKit.h>
#import <TargetConditionals.h>
#include <stdbool.h>
#include <stdint.h>
#include <ifaddrs.h>
#include <arpa/inet.h>
#include <net/if.h>
#include <string.h>

#if TARGET_OS_OSX
#import <AppKit/AppKit.h>
#import <SystemConfiguration/SystemConfiguration.h>
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

@class PatinaAirPlayPlayer;
static NSMapTable<NSNumber *, PatinaAirPlayPlayer *> *PatinaPlayers;
static NSHashTable *PatinaRouteButtons;
static void PatinaBindRoutePickers(uint64_t owner);

@interface PatinaAirPlayPlayer : PlatformView <AVRoutePickerViewDelegate>
- (instancetype)initWithContext:(const void *)context emit:(StateCallback)emit poll:(void (*)(const void *))poll;
- (void)remote:(uint8_t)command value:(int16_t)value;
- (void)disconnect;
- (void)setSource:(NSURL *)source;
- (void)command:(ProbeCommand)command;
- (void)scheduleRefresh;
- (void)invalidate;
- (void)setOwnerID:(uint64_t)owner;
- (AVPlayer *)routingPlayer;
- (void)routesWillPresent;
- (void)routesDidEnd;
@end

@implementation PatinaAirPlayPlayer {
    NSTimer *_timer;
    void (*_poll)(const void *);
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
    uint64_t _ownerID;
    BOOL _routeChoiceArmed;
    BOOL _handoffRequested;
    BOOL _routeUIOpen;
    BOOL _routeWasActive;
    NSTimeInterval _routeSwitchDeadline;
    NSUInteger _routeChoiceSerial;
    NSTimeInterval _routeRecentlyClosed;
    NSURL *_pickerFrameURL;
    AVPlayerItem *_pickerFrame;
#if !TARGET_OS_OSX
    NSString *_pickerOpeningRoute;
    BOOL _audioSessionActive;
#endif
}

- (instancetype)initWithContext:(const void *)context emit:(StateCallback)emit poll:(void (*)(const void *))poll {
    self = [super initWithFrame:CGRectZero];
    if (!self) return nil;
    _context = context;
    _emit = emit;
    _poll = poll;
    __weak PatinaAirPlayPlayer *weakSelf = self;
    _timer = [NSTimer scheduledTimerWithTimeInterval:0.05 repeats:YES block:^(NSTimer *timer __unused) {
        PatinaAirPlayPlayer *owner = weakSelf;
        if (owner && owner->_emit && owner->_poll) owner->_poll(owner->_context);
    }];
    [[NSRunLoop mainRunLoop] addTimer:_timer forMode:NSRunLoopCommonModes];
    _intent = ProbeStop;
    _failedSources = [NSMutableSet set];
    _picker = [[AVRoutePickerView alloc] initWithFrame:self.bounds];
    _picker.delegate = self;
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
    PatinaBindRoutePickers(_ownerID);
}

- (AVPlayer *)routingPlayer { return _player; }

- (void)setOwnerID:(uint64_t)owner {
    if (!PatinaPlayers) PatinaPlayers = [NSMapTable strongToWeakObjectsMapTable];
    if (_ownerID && [PatinaPlayers objectForKey:@(_ownerID)] == self)
        [PatinaPlayers removeObjectForKey:@(_ownerID)];
    uint64_t previous = _ownerID;
    _ownerID = owner;
    if (owner) [PatinaPlayers setObject:self forKey:@(owner)];
    PatinaBindRoutePickers(previous);
    PatinaBindRoutePickers(owner);
}

// AVKit may require a playable item on this specific player to negotiate a
// video route. Keep the current channel on its present output while choosing.
// One paused 16x16 black H.264 frame, generated locally from ffmpeg's color
// source (one baseline frame, no audio, encoder SEI or source metadata).
- (void)primeRoutePicker {
    if (_source || _player.currentItem) return;
    if (!_pickerFrameURL) {
        NSData *frame = [[NSData alloc] initWithBase64EncodedString:
            @"AAAAIGZ0eXBpc29tAAACAGlzb21pc28yYXZjMW1wNDEAAALqbW9vdgAAAGxtdmhkAAAAAAAAAAAAAAAAAAAD6AAA"
            @"A+gAAQAAAQAAAAAAAAAAAAAAAAEAAAAAAAAAAAAAAAAAAAABAAAAAAAAAAAAAAAAAABAAAAAAAAAAAAAAAAAAAAA"
            @"AAAAAAAAAAAAAAAAAAAAAgAAAjl0cmFrAAAAXHRraGQAAAADAAAAAAAAAAAAAAABAAAAAAAAA+gAAAAAAAAAAAAA"
            @"AAAAAAAAAAEAAAAAAAAAAAAAAAAAAAABAAAAAAAAAAAAAAAAAABAAAAAABAAAAAQAAAAAAAkZWR0cwAAABxlbHN0"
            @"AAAAAAAAAAEAAAPoAAAAAAABAAAAAAGxbWRpYQAAACBtZGhkAAAAAAAAAAAAAAAAAABAAAAAQABVxAAAAAAALWhk"
            @"bHIAAAAAAAAAAHZpZGUAAAAAAAAAAAAAAABWaWRlb0hhbmRsZXIAAAABXG1pbmYAAAAUdm1oZAAAAAEAAAAAAAAA"
            @"AAAAACRkaW5mAAAAHGRyZWYAAAAAAAAAAQAAAAx1cmwgAAAAAQAAARxzdGJsAAAAuHN0c2QAAAAAAAAAAQAAAKhh"
            @"dmMxAAAAAAAAAAEAAAAAAAAAAAAAAAAAAAAAABAAEABIAAAASAAAAAAAAAABDExhdmMgbGlieDI2NAAAAAAAAAAA"
            @"AAAAAAAAAAAAAAAAGP//AAAALmF2Y0MBQsAK/+EAFmdCwArZHsBEAAADAAQAAAMACDxImSABAAVoy4PLIAAAABBw"
            @"YXNwAAAAAQAAAAEAAAAUYnRydAAAAAAAAACQAAAAAAAAABhzdHRzAAAAAAAAAAEAAAABAABAAAAAABxzdHNjAAAA"
            @"AAAAAAEAAAABAAAAAQAAAAEAAAAUc3RzegAAAAAAAAASAAAAAQAAABRzdGNvAAAAAAAAAAEAAAMaAAAAPXVkdGEA"
            @"AAA1bWV0YQAAAAAAAAAhaGRscgAAAAAAAAAAbWRpcmFwcGwAAAAAAAAAAAAAAAAIaWxzdAAAAAhmcmVlAAAAGm1k"
            @"YXQAAAAOZYiEBX///w9FAAFC34A="
            options:0];
        NSURL *file = [[NSURL fileURLWithPath:NSTemporaryDirectory() isDirectory:YES]
            URLByAppendingPathComponent:[NSUUID.UUID.UUIDString stringByAppendingPathExtension:@"mp4"]];
        if (![frame writeToURL:file options:NSDataWritingWithoutOverwriting error:NULL]) return;
        _pickerFrameURL = file;
    }
    _pickerFrame = [AVPlayerItem playerItemWithURL:_pickerFrameURL];
    [_player replaceCurrentItemWithPlayerItem:_pickerFrame];
}

#if !TARGET_OS_OSX
- (NSString *)airPlayOutput {
    for (AVAudioSessionPortDescription *port in [AVAudioSession sharedInstance].currentRoute.outputs) {
        if ([port.portType isEqualToString:AVAudioSessionPortAirPlay]) return port.UID;
    }
    return nil;
}
#endif

- (uint64_t)choiceSerial { return _routeChoiceSerial; }

// Late UI replies may arrive after a newer picker or after AVPlayer has
// replaced the priming frame with an actual channel. Neither is ours to stop.
- (void)finishPickerChoice:(uint64_t)serial {
    if (serial && serial == _routeChoiceSerial && !_source &&
        _pickerFrame && _player.currentItem == _pickerFrame) {
        [self disconnect];
    }
}

- (void)expireRouteChoice {
    _routeChoiceArmed = NO;
    [self finishPickerChoice:_routeChoiceSerial];
}

- (void)routesWillPresent {
    if (!_ownerID || !_emit) return;
    _routeUIOpen = YES;
    _routeWasActive = _routeWasActive || (_source && _player.currentItem && (_wasRouted || _player.externalPlaybackActive));
    ++_routeChoiceSerial;
    _routeChoiceArmed = YES;
#if TARGET_OS_OSX
    [self primeRoutePicker];
#else
    _pickerOpeningRoute = [self airPlayOutput];
#endif
    _emit(_context, false, 6);
}

- (void)routesDidEnd {
    _routeUIOpen = NO;
    _routeRecentlyClosed = [NSDate timeIntervalSinceReferenceDate];
    _routeSwitchDeadline = _routeRecentlyClosed + 90.0;
    if (_ownerID && _emit) _emit(_context, false, 7);
    NSUInteger serial = _routeChoiceSerial;
    __weak PatinaAirPlayPlayer *weakSelf = self;
    for (NSNumber *delay in @[@0.5, @2.1, @90.0]) {
        dispatch_after(dispatch_time(DISPATCH_TIME_NOW, (int64_t)(delay.doubleValue * NSEC_PER_SEC)), dispatch_get_main_queue(), ^{
            PatinaAirPlayPlayer *owner = weakSelf;
            if (!owner || owner->_routeChoiceSerial != serial) return;
            if (delay.doubleValue == 90.0) [owner expireRouteChoice];
            [owner refresh];
        });
    }
    [self scheduleRefresh];
}

- (void)routePickerViewWillBeginPresentingRoutes:(AVRoutePickerView *)picker { [self routesWillPresent]; }
- (void)routePickerViewDidEndPresentingRoutes:(AVRoutePickerView *)picker { [self routesDidEnd]; }

- (void)publish:(ProbeState)state external:(BOOL)external {
    if (external && (state == ProbePlaying || state == ProbePaused)) _handoffRequested = NO;
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

// A pre-existing audio route or an audio-only destination cannot authorize
// moving the live stream. Require external video on this player's local frame.
- (void)confirmPickerRoute:(BOOL)changedRoute externalVideo:(BOOL)external {
    if (_ownerID && _routeChoiceArmed && changedRoute && external && _intent == ProbeStop &&
        !_source && _pickerFrame && _player.currentItem == _pickerFrame) {
        _routeChoiceArmed = NO;
        _handoffRequested = YES;
        _emit(_context, true, 8);
    }
}

- (void)refresh {
    if (!_emit) return;
    BOOL external = _player.externalPlaybackActive;
    BOOL routed = external;
    BOOL changedRoute = YES;
#if !TARGET_OS_OSX
    NSString *output = [self airPlayOutput];
    routed = output != nil;
    changedRoute = routed && ![output isEqualToString:_pickerOpeningRoute];
    external = external && routed;
    if (_routeChoiceArmed && changedRoute) [self primeRoutePicker];
#endif
    [self confirmPickerRoute:changedRoute externalVideo:external];
#if !TARGET_OS_OSX
    if (_ownerID && _handoffRequested && !_routeUIOpen && !routed &&
        [NSDate timeIntervalSinceReferenceDate] - _routeRecentlyClosed < 2.0) {
        _handoffRequested = NO;
        _routeChoiceArmed = NO;
        _emit(_context, false, 9);
        [self stopWithState:ProbeIdle];
        _wasRouted = NO;
        [self publish:ProbeIdle external:NO];
        return;
    }
#endif
    if (_ownerID && _routeWasActive) {
        NSTimeInterval now = [NSDate timeIntervalSinceReferenceDate];
        if (_routeUIOpen && (!routed || !external)) {
            // Route changes are multi-step. Do not retire the item on an
            // intermediate KVO event while the user is choosing an output.
            [self publish:ProbeWaiting external:external];
            return;
        }
        if (!_routeUIOpen) {
#if TARGET_OS_OSX
            // A Mac exposes only AVPlayer's video route. Give its final KVO
            // update a main-loop turn before treating a local choice as final.
            BOOL localSettled = !routed && now - _routeRecentlyClosed >= 0.5;
#else
            // On iPad the audio-session route disambiguates local selection
            // from externalPlaybackActive temporarily becoming false.
            BOOL localSettled = !routed;
#endif
            if (localSettled) {
                _routeWasActive = NO;
                _routeChoiceArmed = NO;
                _handoffRequested = NO;
                _emit(_context, false, 9);
                [self stopWithState:ProbeIdle];
                _wasRouted = NO;
                [self publish:ProbeIdle external:NO];
                return;
            }
            if ((!routed || !external) && now < _routeSwitchDeadline) {
                [self publish:ProbeWaiting external:external];
                return;
            }
            if (now - _routeRecentlyClosed >= 2.0) _routeWasActive = NO;
        }
    }
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
        _pickerFrame = nil;
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
    __weak PatinaAirPlayPlayer *weakSelf = self;
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
        case ProbeStop:
            _routeWasActive = NO;
            // The mailbox resets the previous item immediately before Prepare.
            // Preserve the confirmed route until the actual item replaces this one.
            if (_pickerFrame && _player.currentItem == _pickerFrame && _handoffRequested) break;
            [self stopWithState:ProbeIdle];
            break;
    }
    [self refresh];
}

- (void)remote:(uint8_t)command value:(int16_t)value {
    if (command < 3) { [self command:(ProbeCommand)command]; return; }
    switch (command) {
        case 3: _player.volume = MAX(0.0, MIN(1.0, value / 100.0)); break;
        case 4: _player.volume = MAX(0.0, MIN(1.0, _player.volume + value / 100.0)); break;
        case 5: _player.muted = !_player.muted; break;
    }
}

- (void)disconnect {
    ++_routeChoiceSerial;
    _routeWasActive = NO;
    _handoffRequested = NO;
    _routeChoiceArmed = NO;
    _pickerFrame = nil;
    [self stopWithState:ProbeIdle];
    _player.allowsExternalPlayback = NO;
#if TARGET_OS_OSX
    _picker.player = nil;
#endif
    for (NSString *key in @[@"externalPlaybackActive", @"timeControlStatus", @"currentItem.status", @"status"]) {
        [_player removeObserver:self forKeyPath:key context:PlayerObservation];
    }
    // A new, empty AVPlayer releases this app's route without changing the
    // system output device or another application's playback session.
    _player = [AVPlayer new];
    _player.allowsExternalPlayback = YES;
    for (NSString *key in @[@"externalPlaybackActive", @"timeControlStatus", @"currentItem.status", @"status"]) {
        [_player addObserver:self forKeyPath:key options:0 context:PlayerObservation];
    }
#if TARGET_OS_OSX
    _picker.player = _player;
#endif
    _source = nil; [_failedSources removeAllObjects]; _wasRouted = NO; _published = NO;
    PatinaBindRoutePickers(_ownerID);
    [self refresh];
}

- (void)invalidate {
    if (!_emit) return;
    [self setOwnerID:0];
    _picker.delegate = nil;
    [_timer invalidate]; _timer = nil; _poll = NULL;
    _emit = NULL;
    [self setObservedPlayer:nil];
    if (_pickerFrameURL) [[NSFileManager defaultManager] removeItemAtURL:_pickerFrameURL error:NULL];
    _pickerFrameURL = nil;
    _pickerFrame = nil;
#if !TARGET_OS_OSX
    [[NSNotificationCenter defaultCenter] removeObserver:self];
    [self releaseAudioSession];
#endif
}
@end

// Picker controls may be mounted/unmounted with the play bar. They share the
// persistent runtime player and never own a stream or stop it when removed.
@interface PatinaCastButton : AVRoutePickerView <AVRoutePickerViewDelegate>
@property (nonatomic) uint64_t ownerID;
- (void)bindPlayer;
@end
@implementation PatinaCastButton
- (instancetype)initWithFrame:(CGRect)frame {
    self = [super initWithFrame:frame];
    if (!self) return nil;
    self.delegate = self;
#if !TARGET_OS_OSX
    self.prioritizesVideoDevices = YES;
    self.tintColor = UIColor.whiteColor;
#endif
    if (!PatinaRouteButtons) PatinaRouteButtons = [NSHashTable weakObjectsHashTable];
    [PatinaRouteButtons addObject:self];
    return self;
}
- (void)setOwnerID:(uint64_t)owner { _ownerID = owner; [self bindPlayer]; }
- (void)bindPlayer {
#if TARGET_OS_OSX
    self.player = [[PatinaPlayers objectForKey:@(_ownerID)] routingPlayer];
#endif
}
- (void)routePickerViewWillBeginPresentingRoutes:(AVRoutePickerView *)picker {
    [[PatinaPlayers objectForKey:@(_ownerID)] routesWillPresent];
}
- (void)routePickerViewDidEndPresentingRoutes:(AVRoutePickerView *)picker {
    [[PatinaPlayers objectForKey:@(_ownerID)] routesDidEnd];
}
@end
static void PatinaBindRoutePickers(uint64_t owner) {
    if (!owner) return;
    for (PatinaCastButton *button in PatinaRouteButtons) {
        if (button.ownerID == owner) [button bindPlayer];
    }
}
uint64_t patina_airplay_choice_serial(const void *view) {
    return [(__bridge PatinaAirPlayPlayer *)view choiceSerial];
}
void patina_airplay_finish_choice(const void *view, uint64_t serial) {
    [(__bridge PatinaAirPlayPlayer *)view finishPickerChoice:serial];
}
void patina_airplay_owner(const void *view, uint64_t owner) {
    [(__bridge PatinaAirPlayPlayer *)view setOwnerID:owner];
}
void *patina_cast_button_create(void) {
    return (__bridge_retained void *)[[PatinaCastButton alloc] initWithFrame:CGRectZero];
}
void patina_cast_button_owner(const void *view, uint64_t owner) {
    [(__bridge PatinaCastButton *)view setOwnerID:owner];
}
void patina_cast_button_accent(const void *view, uint32_t rgb) {
#if TARGET_OS_OSX
    PatinaCastButton *button = (__bridge PatinaCastButton *)view;
    NSColor *accent = [NSColor colorWithSRGBRed:((rgb >> 16) & 255) / 255.0
        green:((rgb >> 8) & 255) / 255.0 blue:(rgb & 255) / 255.0 alpha:1.0];
    [button setRoutePickerButtonColor:NSColor.whiteColor forState:AVRoutePickerViewButtonStateNormal];
    [button setRoutePickerButtonColor:accent forState:AVRoutePickerViewButtonStateActive];
#else
    ((__bridge PatinaCastButton *)view).activeTintColor = [UIColor colorWithRed:((rgb >> 16) & 255) / 255.0
        green:((rgb >> 8) & 255) / 255.0 blue:(rgb & 255) / 255.0 alpha:1.0];
#endif
}

void *patina_airplay_create(const void *context, StateCallback emit, void (*poll)(const void *)) {
    return (__bridge_retained void *)[[PatinaAirPlayPlayer alloc] initWithContext:context emit:emit poll:poll];
}

void patina_airplay_source(const void *view, const char *source) {
    NSURL *url = source ? [NSURL URLWithString:[NSString stringWithUTF8String:source]] : nil;
    if (![url.scheme isEqualToString:@"http"] || !url.host.length || url.user || url.password) url = nil;
    [(__bridge PatinaAirPlayPlayer *)view setSource:url];
}

void patina_airplay_command(const void *view, uint8_t command) {
    [(__bridge PatinaAirPlayPlayer *)view command:(ProbeCommand)command];
}

void patina_airplay_invalidate(const void *view) {
    [(__bridge PatinaAirPlayPlayer *)view invalidate];
}

void patina_airplay_remote(const void *view, uint8_t command, int16_t value) {
    [(__bridge PatinaAirPlayPlayer *)view remote:command value:value];
}
void patina_airplay_disconnect(const void *view) {
    [(__bridge PatinaAirPlayPlayer *)view disconnect];
}

// No discovery or connection: read only the local interface selected by the OS.
bool patina_airplay_interface(char *buffer, size_t length) {
    NSString *interface = @"en0";
#if TARGET_OS_OSX
    NSDictionary *state = CFBridgingRelease(SCDynamicStoreCopyValue(NULL, CFSTR("State:/Network/Global/IPv4")));
    interface = state[@"PrimaryInterface"];
    if (![interface hasPrefix:@"en"]) return false;
#endif
    struct ifaddrs *addresses = NULL;
    if (getifaddrs(&addresses) != 0) return false;
    bool found = false;
    for (struct ifaddrs *entry = addresses; entry; entry = entry->ifa_next) {
        if (!entry->ifa_addr || entry->ifa_addr->sa_family != AF_INET ||
            !(entry->ifa_flags & IFF_UP) || (entry->ifa_flags & IFF_LOOPBACK) ||
            strcmp(entry->ifa_name, interface.UTF8String) != 0) continue;
        struct sockaddr_in *address = (struct sockaddr_in *)entry->ifa_addr;
        if (inet_ntop(AF_INET, &address->sin_addr, buffer, (socklen_t)length)) { found = true; break; }
    }
    freeifaddrs(addresses);
    return found;
}
