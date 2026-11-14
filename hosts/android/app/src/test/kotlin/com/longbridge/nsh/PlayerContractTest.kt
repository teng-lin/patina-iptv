package com.longbridge.nsh

import androidx.media3.common.Player
import com.hantekllc.iptvrs.host.PipButton
import com.hantekllc.iptvrs.host.PlayerEvent
import com.hantekllc.iptvrs.host.PlayerPip
import com.hantekllc.iptvrs.host.PlayerProps
import com.hantekllc.iptvrs.host.PlayerSource
import com.hantekllc.iptvrs.host.PlayerState
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNotEquals
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test
import java.io.File

/**
 * This host's side of the `tv.player` contract against the fixtures the Rust
 * contract reads and writes exactly (`crates/iptv-player-mpv/fixtures`).
 * Props come through the decoder that hands Rust's props to
 * `PlayerComponent.setProp`, and through the `setProp` it delegates to.
 */
class PlayerContractTest {
    private fun fixture(name: String): Any? = Json.parse(File("../../../crates/iptv-player-mpv/fixtures/$name").readText())

    private fun props() = fixture("player-props.json") as Map<*, *>

    /** What each setter of [PlayerProps] was given, by the prop it is for. */
    private class Seen : PlayerProps {
        val props = linkedMapOf<String, Any?>()

        private fun put(
            prop: String,
            value: Any?,
        ) {
            assertFalse("$prop was set twice", prop in props)
            props[prop] = value
        }

        override fun source(source: PlayerSource?) = put("source", source)

        override fun paused(paused: Boolean) = put("paused", paused)

        override fun pictureSize(mode: String) = put("pictureSize", mode)

        override fun videoTrack(id: String) = put("videoTrack", id)

        override fun audioTrack(id: String) = put("audioTrack", id)

        override fun textTrack(id: String) = put("textTrack", id)

        override fun volume(level: Int) = put("volume", level)

        override fun muted(muted: Boolean) = put("muted", muted)

        override fun audioDelay(ms: Long) = put("audioDelay", ms)

        override fun pip(pip: PlayerPip?) = put("pip", pip)
    }

    /** The fixture's props as the setters must receive them. */
    private val expected =
        mapOf<String, Any?>(
            "source" to
                PlayerSource(
                    url = "http://127.0.0.1:18080/synthetic/index.m3u8",
                    publicOnly = false,
                    headers = mapOf("X-Synthetic" to "fixture"),
                    token = 37L,
                    live = true,
                    reconnect = true,
                ),
            "paused" to true,
            "pictureSize" to "zoom",
            "videoTrack" to "1",
            "audioTrack" to "2",
            "textTrack" to "3",
            "volume" to 80,
            "muted" to true,
            "audioDelay" to 125L,
            "pip" to PlayerPip(auto = true, enter = 3L, window = true),
        )

    /**
     * Every key of the fixture is a prop `setProp` knows (a prop Rust gains
     * fails here until the host has it), and every setter the host has got
     * its value from one (a prop the app no longer sends fails here too).
     */
    @Test fun everyFixturePropReachesItsSetterWithItsValue() {
        val seen = Seen()
        for ((name, value) in props()) {
            assertTrue("the host has no prop named $name", seen.setProp(name as String, value))
        }
        assertEquals(expected, seen.props)
    }

    /**
     * The same for the fields inside `source` and `pip`. No fixture value is
     * what the host falls back to, so a field read under another name than
     * the fixture's cannot pass by its default.
     */
    @Test fun nestedFixtureFieldsAreTheOnesTheHostReads() {
        val props = props()
        assertEquals(
            setOf("url", "publicOnly", "headers", "token", "live", "reconnect"),
            (props["source"] as Map<*, *>).keys,
        )
        assertEquals(setOf("auto", "enter", "window"), (props["pip"] as Map<*, *>).keys)

        val source = expected["source"] as PlayerSource
        val bare = PlayerSource.from(emptyMap<String, Any?>())!!
        assertNotEquals(bare.url, source.url)
        assertNotEquals(bare.publicOnly, source.publicOnly)
        assertNotEquals(bare.headers, source.headers)
        assertNotEquals(bare.token, source.token)
        assertNotEquals(bare.live, source.live)
        assertNotEquals(bare.reconnect, source.reconnect)
        val pip = expected["pip"] as PlayerPip
        val none = PlayerPip.from(emptyMap<String, Any?>())!!
        assertNotEquals(none.auto, pip.auto)
        assertNotEquals(none.enter, pip.enter)
        assertNotEquals(none.window, pip.window)

        val defaults = Seen()
        for (name in props.keys) {
            assertTrue("the host has no prop named $name", defaults.setProp(name as String, null))
        }
        for ((name, value) in expected) {
            assertNotEquals("$name is its default in the fixture", defaults.props[name], value)
        }
    }

    /** A removed prop (null) and a value of another type read as the default. */
    @Test fun aRemovedOrMistypedPropIsItsDefault() {
        val defaults =
            mapOf<String, Any?>(
                "source" to null,
                "paused" to false,
                "pictureSize" to "normal",
                "videoTrack" to "auto",
                "audioTrack" to "auto",
                "textTrack" to "off",
                "volume" to 100,
                "muted" to false,
                "audioDelay" to 0L,
                "pip" to null,
            )
        val removed = Seen()
        for (name in defaults.keys) assertTrue(removed.setProp(name, null))
        assertEquals(defaults, removed.props)
        val mistyped = Seen()
        for (name in defaults.keys) assertTrue(mistyped.setProp(name, listOf(1)))
        assertEquals(defaults, mistyped.props)

        // A header that is not text is dropped, the others stay.
        val url = "http://127.0.0.1:18080/synthetic/index.m3u8"
        assertEquals(
            PlayerSource(url, true, mapOf("X-Synthetic" to "one"), 0L, false, false),
            PlayerSource.from(mapOf("url" to url, "headers" to mapOf("X-Synthetic" to "one", "Bad" to 3))),
        )
    }

    /**
     * A `source` object without an address (or with one that is not text)
     * is still the player's source: it plays nothing, but its token,
     * headers, `live` and `publicOnly` reach `setSource` as they always
     * did, where they decide whether the transport is closed and reset.
     * Only JSON null, or a value that is no object, is no source.
     */
    @Test fun aSourceWithoutAnAddressKeepsItsOtherFields() {
        val fields =
            mapOf(
                "publicOnly" to false,
                "headers" to mapOf("X-Synthetic" to "fixture"),
                "token" to 37,
                "live" to true,
                "reconnect" to true,
            )
        val kept = PlayerSource(null, false, mapOf("X-Synthetic" to "fixture"), 37L, true, true)
        for (source in listOf(fields, fields + ("url" to null), fields + ("url" to 7))) {
            val seen = Seen()
            assertTrue(seen.setProp("source", source))
            assertEquals("$source", mapOf<String, Any?>("source" to kept), seen.props)
        }
        // With nothing else in it, the defaults of a source: not of none.
        assertEquals(
            PlayerSource(null, true, emptyMap(), 0L, false, false),
            PlayerSource.from(emptyMap<String, Any?>()),
        )
        for (none in listOf(null, "http://127.0.0.1:18080/synthetic/index.m3u8", listOf(1), 0)) {
            assertNull("$none", PlayerSource.from(none))
        }
    }

    /**
     * Media3's four playback states are the contract's. Any other number
     * (Media3 has none today) is still sent, as `unknown`, which is no
     * state of the contract and so not in the shared fixture: the app shows
     * that name on its status line and does nothing else.
     */
    @Test fun aMedia3StateOutsideTheContractIsStillSent() {
        assertEquals(PlayerState.IDLE, PlayerState.of(Player.STATE_IDLE))
        assertEquals(PlayerState.BUFFERING, PlayerState.of(Player.STATE_BUFFERING))
        assertEquals(PlayerState.READY, PlayerState.of(Player.STATE_READY))
        assertEquals(PlayerState.ENDED, PlayerState.of(Player.STATE_ENDED))
        for (other in listOf(0, 5, -1, Int.MAX_VALUE)) {
            assertEquals("$other", PlayerState.UNKNOWN, PlayerState.of(other))
        }
        val event = PlayerEvent.State(PlayerState.UNKNOWN)
        assertEquals("state", event.name)
        assertEquals(mapOf<String, Any?>("state" to "unknown"), event.data())
        val states =
            (fixture("player-events.json") as List<*>)
                .map { it as Map<*, *> }
                .filter { it["name"] == "state" }
                .map { (it["data"] as Map<*, *>)["state"] }
        assertFalse(PlayerState.UNKNOWN.wire in states)
    }

    /** `userAgent`, which the players once took, is no prop: nothing is set. */
    @Test fun aNameOutsideTheContractSetsNothing() {
        val seen = Seen()
        for (name in listOf("userAgent", "user-agent", "Source", "picture_size", "")) {
            assertFalse("$name is not a prop", seen.setProp(name, "SyntheticAgent/1.0"))
        }
        assertTrue(seen.props.isEmpty())
    }

    /**
     * JSON as the Rust decoder tells it apart: a number is a number whatever
     * Java type carries it, and a null field is a missing one.
     */
    private fun canon(value: Any?): Any? =
        when (value) {
            is Map<*, *> -> value.filterValues { it != null }.mapValues { canon(it.value) }
            is List<*> -> value.map { canon(it) }
            is Number -> value.toDouble()
            else -> value
        }

    /**
     * Each event of the fixture, which Rust decodes into its typed event, is
     * what this host's typed event of the same value emits: the same name
     * and the same data, in the fixture's order.
     */
    @Test fun everyFixtureEventIsWhatTheHostEmits() {
        val tracks =
            PlayerEvent.Tracks(
                video =
                    listOf(
                        PlayerEvent.Tracks.Video("0:0", "HD", 1280, 720, 50f, 2_500_000),
                        PlayerEvent.Tracks.Video("0:1", "", null, null, null, null),
                    ),
                audio =
                    listOf(
                        PlayerEvent.Tracks.Audio("1:0", "Commentary", "eng", 2, "audio/mp4a-latm"),
                        PlayerEvent.Tracks.Audio("1:1", "", "", null, ""),
                    ),
                text = listOf(PlayerEvent.Tracks.Text("2:0", "Synthetic", "fre")),
                selectedVideo = "0:1",
                selectedAudio = "1:0",
                selectedText = "2:0",
            )
        val events: List<PlayerEvent> =
            listOf(
                PlayerEvent.State(PlayerState.IDLE),
                PlayerEvent.State(PlayerState.BUFFERING),
                PlayerEvent.State(PlayerState.READY),
                PlayerEvent.State(PlayerState.ENDED),
                PlayerEvent.Size(1280, 720),
                PlayerEvent.Error("ERROR_CODE_IO_BAD_HTTP_STATUS", 404),
                PlayerEvent.Error("ERROR_CODE_IO_NETWORK_CONNECTION_FAILED"),
                PlayerEvent.Position(4000L),
                tracks,
                PlayerEvent.Tracks(emptyList(), emptyList(), emptyList(), "auto", null, "off"),
                PlayerEvent.Format(25f, 2),
                PlayerEvent.Format(50f, null),
                PlayerEvent.Format(null, 6),
                PlayerEvent.Pip(supported = true, tv = false, active = true),
                PlayerEvent.PipAction(PipButton.PLAY),
                PlayerEvent.PipAction(PipButton.PAUSE),
                PlayerEvent.PipAction(PipButton.NEXT),
                PlayerEvent.PipAction(PipButton.PREVIOUS),
            )
        val fixture = fixture("player-events.json") as List<*>
        assertEquals(fixture.size, events.size)
        for ((index, event) in events.withIndex()) {
            val entry = fixture[index] as Map<*, *>
            assertEquals(setOf("name", "data"), entry.keys)
            assertEquals("event $index", entry["name"], event.name)
            assertEquals("event $index (${event.name})", canon(entry["data"]), canon(event.data()))
        }
        // Every state of the contract and every button the host can name
        // is in the fixture.
        assertEquals(
            PlayerState.entries.toSet() - PlayerState.UNKNOWN,
            events.filterIsInstance<PlayerEvent.State>().map { it.state }.toSet(),
        )
        assertEquals(
            PipButton.entries.map { it.wire }.toSet(),
            events.filterIsInstance<PlayerEvent.PipAction>().map { it.action }.toSet(),
        )
        for (button in PipButton.entries) assertEquals(button, PipButton.of(button.wire))
        assertNull(PipButton.of("unknown"))
        assertNull(PipButton.of(null))
        assertEquals(mapOf("action" to "unknown"), PlayerEvent.PipAction("unknown").data())
        // Nothing known is no `format` event; one known field is.
        assertTrue(PlayerEvent.Format(null, null).isEmpty)
        assertFalse(PlayerEvent.Format(null, 2).isEmpty)
    }
}
