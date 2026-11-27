# Signed macOS updates

GitHub release discovery and release notes work independently of Sparkle. The
current ad-hoc signed previews offer their matching DMG for manual installation.
They cannot safely replace themselves through the signed production channel.

Patina includes an opt-in bridge to Sparkle **2.10.1**, pinned to the upstream
archive SHA256 in `tools/ci/macos-sparkle.py`. Sparkle supplies download progress,
cancellation, Ed25519 verification, app replacement and relaunch. Patina owns
check frequency and requires an explicit installation action. Sparkle's own
automatic checks, automatic downloading/installing and JavaScript release notes
are disabled. Its appcast version must match the GitHub version the viewer chose.
Relaunch waits until the recording host can close safely and flushes watch time.
AppKit publishes a token-scoped restart request; the owning NSH shell task checks
the runtime and approves it. Native callbacks never access shell state. Disposing
that root revokes queued approval. A forwarding public `SPUUserDriver` preserves
Sparkle's standard UI and captures its real check/download cancellation blocks.
Cancellation ends when extraction begins, matching Sparkle's standard UI. The app
hides Cancel and disables Skip during extraction and preparation; Later can still
close the app panel. The public Ready reply is held until the recording guard
approves installation, including when resuming an already prepared installer.
Root disposal before approval answers that one-shot reply with Skip, which cancels
the prepared installation; disposal during extraction retains only native state
until Ready can answer Skip. Version validation covers resumed updates too.

Once the approved Install reply runs, Sparkle's external installer owns the
handoff. A prepared update can install on a later normal quit even when immediate
termination is declined. The external agent observes process exit; it does not
require Cocoa's WillTerminate notification. Patina keeps its existing quit and
recording guards. If the approved process remains alive after 60 seconds, Patina
releases the native UI/controller before restoring the recording host and reports
that installation was deferred. This does not claim to remove the prepared update.
Checking again can resume it through the same version and recording guards.

## Public configuration and packaging

Before enabling production updates, provision an Ed25519 update-signing key in
the protected release publisher, establish the HTTPS appcast URL, and complete
the existing Developer ID signing/notarization setup. The update key is separate
from the Apple signing identity and the TUF key used by other installer adapters.
No production updater key, appcast URL or Developer ID credential is supplied by
this change. Do not generate a throwaway key for production or alter the preview
signing policy to make capability checks pass.

Set repository **variables** `PATINA_SPARKLE_FEED` and
`PATINA_SPARKLE_PUBLIC_KEY` together. The unsigned macOS candidate job downloads
the pinned upstream archive; bundling verifies its hash and framework signature,
copies the framework and its license, and seals the public configuration inside
the app. Partial configuration fails packaging. Neither variable is a secret.

For a local candidate build, the equivalent command is:

```sh
PATINA_SPARKLE_ARCHIVE=/outside-checkout/Sparkle-2.10.1.tar.xz \
PATINA_SPARKLE_FEED=https://updates.example.test/patina/appcast.xml \
PATINA_SPARKLE_PUBLIC_KEY="$PATINA_PUBLIC_UPDATE_KEY" \
bash tools/package-macos.sh
```

The existing protected signing stage must sign the nested Sparkle helper bundles
inside out, preserving their entitlements, then sign/notarize the enclosing app
and DMG. Patina enables the action only for a running Developer ID application
with complete public configuration and the pinned bundled framework. An unsigned
candidate remains manual even when it contains Sparkle.

## Publishing

After the final DMG is signed, notarized and verified, publish that immutable DMG
to its GitHub release. Generate an appcast from those exact bytes using the
**pinned** upstream `bin/generate_appcast`; use its `--ed-key-file` option with a
protected file outside the checkout. Match `--download-url-prefix` to the exact
GitHub release asset directory and include release notes beside the DMG. Publish
the resulting appcast at `PATINA_SPARKLE_FEED` after the referenced assets exist.
Retain the appcast and artifact hashes as release evidence. Never place the
private key in repository variables, candidate artifacts, app resources or logs.

The general release workflow does not publish an appcast without this provisioned
publisher. Missing or mismatched appcast entries fail installation; GitHub notes
and the manual package route remain usable.

## Upgrade acceptance

`python3 tools/ci/test-macos-dmg.py` checks absent/partial public configuration,
transport/key validation, the pinned archive boundary, version preservation and
disabled automatic installation. On macOS it also compiles the actual bridge
against AppKit/Security and checks preview refusal, appcast/GitHub version
agreement, requests from a separate native thread, shell-authorized relaunch,
root disposal after queued approval, genuine download cancellation, extraction
races, prepared Skip replies, resumed-version rejection, retry reauthorization,
and generation/attempt-fenced deferred recovery. A separate real AppKit child
process verifies that a delegate can cancel Cocoa termination, stop its run loop,
and exit normally without a WillTerminate callback, as the pinned NSH runner does.

Before enabling production, stage two differently versioned Developer ID builds
using the same public updater key. Install the older build with a synthetic
encrypted library and recording, discover the newer GitHub release, cancel one
download, reject a tampered archive, then complete the verified upgrade. Verify
the newer version after relaunch, reopen the existing encrypted library, retain
recordings and exercise backup restore. Start a recording while the installation
dialog is open and verify that relaunch waits. Cancel at the download/extraction
boundary and dispose the root before approving a prepared update; verify that the
public Skip reply cancels it. Decline an immediate quit and confirm the recording
host resumes, the deferred notice appears, and a later check resumes only the
selected version. Exercise an unwritable installation
and offline appcast. These signed install-over-previous checks require the real
protected signing setup; the synthetic bridge test does not claim to replace them.

Upstream references: [programmatic setup](https://sparkle-project.org/documentation/programmatic-setup/),
[publishing](https://sparkle-project.org/documentation/publishing/), and
[code signing](https://sparkle-project.org/documentation/#6-code-sign-your-app).

The lifecycle contract follows the pinned public
[SPUUserDriver](https://github.com/sparkle-project/Sparkle/blob/2.10.1/Sparkle/SPUUserDriver.h)
replies and the external
[InstallerProgress](https://github.com/sparkle-project/Sparkle/blob/2.10.1/Sparkle/InstallerProgress/InstallerProgressAppController.m)
termination observer. It uses no private Sparkle driver API or AppKit delegate replacement.
