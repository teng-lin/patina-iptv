# Casting through Patina

Choose **Cast to TV** while playing a live HLS channel. Patina creates a
session URL for the selected receiver and keeps the upstream credentials and
request headers on the sending device. Keep Patina running and connected
while the receiver plays. Choose **Play here** to return to local playback.

Apple hosts use the system AirPlay picker. The experimental Linux AirPlay
sender can be enabled by starting Patina with `PATINA_RUST_AIRPLAY=1`. Its
destinations carry an AirPlay label. On Android and Linux, Patina connects
without a code when the receiver accepts a temporary session. If the receiver
requires a PIN, approve pairing on the receiver and enter the four-digit code
it shows. Patina stores verified pairings on Android and in the current Linux
user's Secret Service. If a paired receiver's identity changes, Patina asks
before forgetting the stored pairing.

`PATINA_CAST_RELAY_PORT` controls the sender's TCP listener and defaults to
49152. Linux AirPlay also has a UDP timing listener. Set
`PATINA_AIRPLAY_TIMING_PORT` to choose its port or leave it unset for an
ephemeral port. Use the port in the failure message when checking receiver
reachability. The relay restricts fetches to the selected peer and the
session URL. Keep the URL private.

An AirPlay receiver may accept the request before it starts playing.
Patina publishes the handoff after it sees owned playback. Controls appear
when the session reports them available. When a receiver supports only
closing the session's transport, Patina reports that remote stop could
not be confirmed and retires the relay before reopening local playback.
