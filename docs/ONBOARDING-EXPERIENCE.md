# Onboarding experience and rehearsal

The experience starts with relief, then shows how Passband organizes mail:

1. **You're only human.** “Inbox zero every day was never realistic. You're
   only human.” The supporting line reminds you that your attention is valuable. A small pile of
   example messages accompanies the introduction.
2. **A different goal.** “Know what needs you.”
   The same cards organize into a request, a record, and a newsletter. Both
   opening beats are user-paced, with Back and Skip intro controls.
3. **Bring your inbox.** Choose Log in or Sign up, with Self-hosted login as a secondary option.
   Log in opens the browser and leaves a full manual pairing form available;
   a deep link emphasizes device and account names and collapses technical details.
   Hosted auth URLs carry the current light/dark appearance via the theme parameter
   introduced in #214, including when the app follows the system appearance.
   Pairing links bypass the opening and go straight to connection confirmation.
4. **Explore a practice inbox.** The moment the account connects — before a
   single live message is fetched or drawn — the app holds the board behind a
   loading gate, fills a practice mailbox in the same window and the same
   shell, and reveals it with a welcome card over the blurred board: three
   example messages in each category. The welcome card can be declined with
   **Skip the guide** or Escape. A guided tour introduces Needs You, Calendar,
   Shipments, and Newsletters, with Banking and Receipts highlighted together
   as other useful categories.
5. **Try the actions.** Read Maya's no-reply acknowledgement, press **E** to mark it done, and undo
   that action. For Your Eyes works like a to-do list: mail stays until you mark
   it done. Open any calendar example, then use **Escape** to advance. Shipment examples
   are optional: choose Next or open one and return with Escape. Then open Brightly's
   update and create a rule in the real rule editor.
6. **Your real inbox.** Explore my inbox zooms the practice board away and
   puts the connected account back under the same veil. The live summary shows
   emails triaged, noise set aside, and messages needing attention as the
   first real update lands. There is no second product tour. Reduce Motion
   uses a brief fade. Sidebar tabs stay visible but inert during practice,
   with a gentle shake or a stationary outline on attempted navigation.

## One app, one process

Practice is a **mode of the store**, not a second copy of the app.
`AppStore.enterPractice` does what an account switch does — bumps the epoch,
stops the poller and the event feeds, settles drafts, wipes the read model —
and then points the API client at `RehearsalAPI`, an in-memory transport that
answers the human-door routes with fictional mail. `exitPractice` wipes the
fixture world under a new epoch, drops the flag, and re-runs the ordinary boot
path (`loadSettings`), so the live account comes back through the same
`.connected` transition that starts the poller and feeds on any launch.

What that buys, compared with the earlier two-process handoff: no second Dock
icon, no window that can be closed out from under a waiting host, no token
that a restarted host has forgotten, and no throwaway preference domain.
Preferences and credentials are the real ones throughout — the person
practicing is the person who will read the live inbox a minute later, so their
theme, their name and their telemetry identity carry across. The practice
`/client/stats` reports the live account's own address (read from the real
daemon once, before the wipe, since on a first run the live poller has not
pulled yet), so the greeting seeds from the real mailbox and not from the
fixture.

While practice is up every menu command that navigates, composes, asks or
refreshes is disabled along with the account ones, on purpose: navigation is
refused for the duration, a composer could only fail to send, and a greyed
menu is more honest than one that silently does nothing. Leaving practice
restarts the poller and the feeds by hand as well as through the `.connected`
transition, so a keychain read that beats the render loop cannot leave the
summary waiting for an update that never comes.

What is and is not isolated:

- **The API client** answers every `/client/*` call from `RehearsalAPI` while
  the flag is up; unsupported routes fail locally with a practice error.
  Nothing on those paths reaches a server.
- **Networking is not blocked.** The event feeds are stopped for the duration
  and favicon/newsletter-art fetches are skipped, but the reader's WebKit
  process is outside any URL-loading hook, so the fixtures are self-contained
  on purpose: every photograph is a bundled data URL and no fixture references
  a remote resource. Keep it that way when adding one.
- **Keychain** is never read or written during practice because nothing on the
  practice path asks for credentials (the Connect gate and Add Account are
  closed and inert). It is not guarded; there is nothing to guard.
- **Telemetry stays on.** Every event captured while practice is up carries
  `practice: true`, so a fixture `e`, a fixture rule and a fixture undo are
  distinguishable from the real thing without a second vocabulary. The
  onboarding funnel — `tour_started` (with its source: first run, Settings,
  or a rehearsal), `tour_completed` and `tour_skipped` (with the practice step
  reached) — is the one exception and never carries the tag. The daily
  `triage_digest` is skipped in practice so fixture counts cannot burn the
  day's stamp. A standalone rehearsal launch has no analytics client at all.
- **Pair links are dropped, not parked, during practice.** A link held through
  a lesson would raise the Add Account sheet over the summary card with a code
  that had gone stale in a forgotten browser tab.

## Run a fresh rehearsal

From the repository root:

```sh
cd passband
./rehearse-onboarding.sh
```

This creates an unoptimized preview build and launches a separate process in
rehearsal mode. `./build.sh release` remains the optimized build.
Once built, launch another fresh session without rebuilding:

```sh
./rehearse-onboarding.sh --no-build
```

The main window matches customer onboarding: the opening beats, then the
practice inbox with its guide, then the veil and — on a machine with a paired
account — the live summary over the real board. With no account, leaving
practice lands on the Connect gate. A separate **Onboarding rehearsal** window
holds an appearance switch (the app's real theme preference, the same one `\`
flips) and **Start fresh**, which returns to the opening and reloads the
practice mail on the next Continue. Reopen the controls with **Rehearsal →
Show rehearsal controls**. Customer onboarding never opens the control window or shows the demo mailbox
appearance selector. Account setup retains its own theme toggle. The equivalent direct launch is:

```sh
open -n build/Passband.app --args --onboarding-rehearsal
```

`PASSBAND_ONBOARDING_REHEARSAL=1` is also available when launching the binary
from a debugger.

## What this iteration tests

The rehearsal shares the real opening view, then mounts **the actual app shell**:
the same sidebar, dashboard, newsletters, records rail, message reader, and
actions used by a connected account. The guide uses the same read models and
actions as the connected app. It does not simulate Google authorization,
provisioning, or real model inference. Settings can reopen the practice inbox;
`PASSBAND_FORCE_TOUR=1` bypasses its completed flag. The former seven-step tour
and its simulated actions have been removed.

Try these paths when reviewing a change:

- Advance and go back through the opening; skip it; resize the window.
- Open Juniper or any calendar example: confirm the guide changes to **Escape**.
- Press **Escape** to advance to shipments; verify Next works without opening mail.
- Optionally open an ExFed/Rainforest shipment and return with Escape.
- Read Maya's acknowledgement, mark it done with **E**, then use **U** to undo.
  The undo chip waits while you read; skipping the lesson retires it.
- Open Brightly's update and save a mute rule in the actual editor.
- Finish or skip the guide and verify the veil, then the live summary (or the
  Connect gate on a machine with no account).
- Use Start fresh before finishing to repeat practice; verify sidebar tabs cannot navigate.
- Check light/dark appearance, system Reduce Motion, and keyboard navigation.

The poller and rehearsal suites run first in `./test.sh`. They cover preloaded
category contents, reading/done/undo, rule actions, and reset. API tests decode
the same wire types as the real client. The controller suite runs the real
`TourController` against a store stub that flips the real practice flag —
with a `.loading` frame and a suspension point inside its `enterPractice`, so
a cancel landing mid-swap is exercised, not assumed away — and covers the
first-run trigger, skip and completion leaving practice for the summary, the
held undo chip, stale-run callbacks, and the reader lessons.

The fixture transport is a controlled example, not a replacement for an
end-to-end run with a real newly connected account. Features outside its
supported routes report a local rehearsal error instead of contacting a server.
Free-text rule filters can be saved for editor practice, but only explicit
allow/mute dispositions affect the fixtures. Sending mail is unsupported.

Calendar fixture bodies prefetch before the guide. Calendar and shipment
instructions follow loaded reader/close events rather than guide buttons. The
lesson cards teach native keys and row clicks.

Cats Weekly, Haightssion, and the satirical Federal Overstatement have distinct HTML
layouts and bundled inline photographs. They render without remote-image
requests. See [asset prompts and paths](ONBOARDING-NEWSLETTER-ASSETS.md).

Newsletter sender marks are drawn locally in the actual card/reader presenters.
The sync timestamp and refresh chip are hidden only in practice; the real
mailbox retains them.

The guide uses larger, shorter explanations and places its card beside the
measured target where space permits. Otherwise, it rests at the bottom left, just past the sidebar, with clearance for the Undo toast. Next is prominent; Skip stays secondary. Maya’s exact row is highlighted for the reading lesson and uses a bundled fictional portrait.
Brightly gets a row-specific highlight, a smart-rules teaching step, and a saved
rule confirmation. The strong mute confirmation depends on the actual saved
rule suppressing the Brightly example; changed dispositions or patterns receive
a generic confirmation. Calendar and shipment providers are fictional Zip,
ExFed, and Rainforest. Haightssion uses an original sharp geometric mark.

Brightly’s fictional legal notice uses an oversized wordmark and yellow sunburst, with the matching sunburst in its sender avatar. The email includes a dynamic effective date, a summary of changes, and a service-notice footer.

The phone has no practice inbox yet: `PracticeProductTour` is a Mac lesson
over the Mac shell and is excluded from the iOS target. Its guide is still to
be designed; until then iOS onboarding is the live summary alone.

Practice transitions share the account-mutation gate, so outstanding credential operations finish before transport changes. Account menus and mutation entry points reject changes during practice and its transitions. Credential probes explicitly refuse fixture transport. Entry gates refreshes with the loading state and fences the epoch again after draft settlement. Stopping the poller cancels and detaches its current pull, so practice warmup cannot join a live request. Regression suites cover stale poll completion and credential-probe isolation.

### Rehearse account connection

Run `./rehearse-onboarding.sh --with-connection` (optionally add `--no-build`) to start at the real intro and account connection screens. Existing saved accounts are preserved; signing in or pairing uses real credential validation and persists the connection through the normal app flow. Immediately after a successful connection, practice starts even if this install completed onboarding previously. Start fresh repeats connection in this mode. The control panel’s **Use copied login link** routes a copied Passband pairing URL to this rehearsal when macOS opens an older registered build instead.

The closing “A little less to carry” card uses the same blurred-mailbox modal as the opening. If a saved connection cannot be restored, exit skips repeated intro slides and lands on the credential form for the KIND of daemon the saved account was (its host prefilled from the index), with the connection error and a **Try saved connection again** retry that survives the form clearing the error and is offered on the welcome screen too. Successful connection continues to the live summary rather than starting practice again.
