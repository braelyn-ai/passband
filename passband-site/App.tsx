import {
  useEffect,
  useMemo,
  useRef,
  useState,
  type FormEvent,
  type MouseEvent,
  type PointerEvent,
} from "react";
import { CARRIERS, createScope, markerPoint, type ScopeLayout } from "./scope";

// The "before" panel is a procedurally repeated fake inbox: the drudgery
// Passband exists to kill. Snippets are written long so rows run the full
// width of the panel.
const FAKE_EMAILS: Array<[sender: string, subject: string, snippet: string]> = [
  ["LinkedIn", "You appeared in 9 searches this week", "See who's looking for someone like you. Recruiters from companies you've never heard of are searching for profiles matching yours..."],
  ["Medium Daily Digest", "Stories for you", "10 Habits of Highly Effective Engineers | 12 min read. Why I Quit My Job to Farm Mushrooms | 8 min read. The Death of the..."],
  ["no-reply@accounts", "Security alert for your account", "A new sign-in was detected on a device you may or may not recognize. If this was you, no action is needed. If this wasn't..."],
  ["DoorDash", "Your Friday deserves 40% off", "Hungry? Use code FRIYAY at checkout before midnight and get 40% off orders over $35, up to a maximum discount of $8..."],
  ["Confluence", "Weekly digest: 47 updates in spaces you follow", "Q3 Planning Doc was edited by 6 people. Retro Notes (DRAFT) (COPY) was moved. A page you commented on in 2024 was..."],
  ["Marriott Bonvoy", "Points update: you have 312 points", "Your points balance summary for the month. You are 87,688 points away from your next free night at participating..."],
  ["Product Hunt Daily", "The 10 best new products today", "An AI notetaker for your AI notetaker, a smart water bottle that syncs to your calendar, and 8 more launches you'll..."],
  ["billing@saas.io", "Your receipt from Acme SaaS #48211", "Amount paid: $12.00. Thank you for your continued subscription to a service you forgot you signed up for. Manage your..."],
  ["United Airlines", "MileagePlus: miles expiring soon", "Don't lose your 1,204 miles. Book by the end of the month or transfer them to a partner at an exchange rate that will..."],
  ["HR Announcements", "REMINDER: Open enrollment closes Friday", "This is your final reminder to complete your benefits elections. If you do not act, your current elections will roll..."],
  ["Substack", "3 new posts from writers you follow", "The Case Against Breakfast, and other essays. Plus: a 4,000-word post about someone's move to Lisbon and what it..."],
  ["Twitter", "You have 4 new notifications", "@someguy and 3 others liked a post you were mentioned in. See what you're missing. Your network has been busy while..."],
  ["Zoom", "Your cloud recording is now available", "Meeting recording: Weekly Sync. Duration: 58 minutes. This recording will be automatically deleted in 30 days and..."],
  ["Chase", "Your statement is ready", "Your February statement for account ending in 4482 is now available. Your minimum payment is due in 21 days. Log in..."],
  ["GitHub", "[repo] 23 new notifications", "dependabot opened 14 pull requests in repositories you have not touched since 2023. Bump lodash from 4.17.20 to..."],
  ["Eventbrite", "Events near you this weekend", "Networking mixers, fun runs, and a pottery class. Based on your interests: 12 events happening within 25 miles of..."],
  ["Google Calendar", "Daily agenda for Tuesday", "You have 7 events scheduled today starting with Standup at 9:00 AM, followed by a meeting that could have been an..."],
  ["Sephora", "We miss you! Here's 15% off", "It's been a while. Your Beauty Insider points are waiting, and so is 15% off your next purchase of $50 or more..."],
  ["The Team", "We've updated our Privacy Policy", "We're writing to let you know about some updates to our Privacy Policy and Terms of Service, effective in 30 days..."],
  ["Slack", "You have unread messages in #general", "While you were away, 312 messages were posted in channels you follow, including a heated thread about the office..."],
];
// The same inbox's mail that mattered, buried in the scroll exactly as it would
// be. These are the rows the "after" panel pulls forward, so the two panels are
// honestly one mailbox before and after, not two different ones.
const BURIED: Array<[sender: string, subject: string, snippet: string]> = [
  ["Jamie Chen", "A quick decision before Friday", "Hey! Two options for the offsite venue, I need your pick by Friday so we can hold the date..."],
  ["Parkline Properties", "Lease renewal: signature needed", "Your renewal packet is ready. Please review and sign by September 23 to lock in your current rate..."],
  ["UPS", "Your package is out for delivery", "Keychron Q1 Max. Scheduled delivery: today by 8:00 PM. Track your package for live updates..."],
  ["Dr. Ortiz's Office", "Please confirm Thursday's appointment", "Reply C to confirm your appointment on Thursday at 2:30 PM, or call us to reschedule..."],
];

// Every colour on the page, lifted from the Swift client's dark palette
// (Design/Palette.swift) so the site and the app are one product. The page is
// always night: the scene is the brand, and its glow version is the one worth
// leading with.
const PAGE_CSS = `
:root {
  --bg: #090D16;
  --canvas: #0E141D;
  --card: #121A26;
  --ink: #E9EEF5;
  --dim: #A8B4C4;
  --faint: #808E9F;
  --faintest: #62707F;
  --hair: rgba(233, 238, 245, 0.08);
  --hair-strong: rgba(233, 238, 245, 0.14);
  --accent: #4E9BEA;
  --accent-ink: #82BAF5;
  --warn: #E8AC4C;
  --positive: #4FC08A;
  --danger: #FF7A68;
  --serif: "Newsreader", ui-serif, Georgia, serif;
  --sans: -apple-system, BlinkMacSystemFont, "SF Pro Text", system-ui, sans-serif;
  --gutter: clamp(1rem, 4vw, 2.75rem);
}
html { background: var(--bg); color-scheme: dark; }
body { margin: 0; background: var(--bg); color: var(--ink); font-family: var(--sans);
  -webkit-font-smoothing: antialiased; }
.pb a { color: inherit; }

/* THE STORY. A tall section with a sticky stage inside it: the scroll through
   the section is the squelch closing, and the stage holds still while it
   happens. The height is the length of the scrub. */
.pb-story { position: relative; height: 260vh; }
.pb-stage { position: sticky; top: 0; height: 100vh; height: 100svh; overflow: hidden; }
.pb-scene { position: absolute; inset: 0; width: 100%; height: 100%; display: block; }
/* The analyzer's graticule: ten divisions across, eight down, faded out at the
   edges so the screen has no hard border to sit inside. */
.pb-grat { position: absolute; inset: 0; pointer-events: none;
  background-image:
    repeating-linear-gradient(90deg, rgba(130, 186, 245, 0.07) 0 1px, transparent 1px 10%),
    repeating-linear-gradient(180deg, rgba(130, 186, 245, 0.07) 0 1px, transparent 1px 12.5%);
  -webkit-mask-image: radial-gradient(ellipse 75% 70% at 62% 55%, #000 30%, transparent 85%);
          mask-image: radial-gradient(ellipse 75% 70% at 62% 55%, #000 30%, transparent 85%); }
/* Markers: numbered like an analyzer's, riding the carriers' peaks. They come
   up only once the squelch has closed, which is when there is something to
   point at. */
.pb-mkr { position: absolute; z-index: 1; transform: translate(-50%, calc(-100% - 12px)); pointer-events: none;
  font: 600 11px/1 ui-monospace, "SF Mono", Menlo, monospace; color: var(--accent-ink);
  display: flex; flex-direction: column; align-items: center; gap: 3px;
  opacity: 0; transition: opacity 0.5s ease; }
.pb-mkr::after { content: ""; border: 5px solid transparent; border-top: 6px solid var(--accent); border-bottom: 0; }
.pb-stage[data-locked] .pb-mkr { opacity: 1; }
.pb-mkr:nth-of-type(2) { transition-delay: 0.08s; } .pb-mkr:nth-of-type(3) { transition-delay: 0.16s; }
.pb-mkrs { position: absolute; z-index: 2; right: var(--gutter); top: 17%; width: 19rem;
  font: 500 12px/1.2 ui-monospace, "SF Mono", Menlo, monospace; color: var(--dim);
  border: 1px solid rgba(130, 186, 245, 0.2); border-radius: 10px; background: rgba(9, 13, 22, 0.7);
  -webkit-backdrop-filter: blur(6px); backdrop-filter: blur(6px);
  opacity: 0; transform: translateY(6px); transition: opacity 0.6s ease 0.2s, transform 0.6s ease 0.2s; }
.pb-stage[data-locked] .pb-mkrs { opacity: 1; transform: none; }
.pb-mkrs div { display: grid; grid-template-columns: 1.6rem 1fr auto; gap: 0.6rem; padding: 0.55rem 0.8rem;
  border-top: 1px solid rgba(130, 186, 245, 0.1); }
.pb-mkrs div:first-child { border-top: 0; font-size: 10px; letter-spacing: 0.1em; color: var(--faintest); }
.pb-mkrs b { color: var(--accent-ink); font-weight: 600; }
.pb-mkrs span:last-child { text-align: right; }
.pb-mkrs .late { color: var(--danger); } .pb-mkrs .soon { color: var(--warn); }
/* The readouts along the screen's foot. SQUELCH is live: it is the scroll. */
.pb-read-l, .pb-read-r { position: absolute; z-index: 2; bottom: 1.4rem;
  font: 500 11px/1 ui-monospace, "SF Mono", Menlo, monospace; letter-spacing: 0.08em; color: var(--faintest); }
.pb-read-l { left: var(--gutter); } .pb-read-r { right: var(--gutter); }
.pb-read-r b { color: var(--accent-ink); font-weight: 600; display: inline-block; min-width: 4ch; text-align: right; }
/* The scrim, the web twin of the intro's: the backdrop's own colour, so it
   melts into the scene rather than tinting it. From the leading edge on wide
   screens, from the top on narrow ones where the copy sits above the waves. */
.pb-scrim { position: absolute; inset: 0; pointer-events: none;
  background:
    linear-gradient(90deg, rgba(9,13,22,0.95) 0%, rgba(9,13,22,0.82) 30%, rgba(9,13,22,0.4) 44%, rgba(9,13,22,0) 60%),
    linear-gradient(180deg, rgba(9,13,22,0) 72%, rgba(9,13,22,0.85) 100%); }
.pb-top { position: absolute; inset: 0 0 auto; display: flex; align-items: center;
  justify-content: space-between; padding: 1.4rem var(--gutter); z-index: 2; }
.pb-brand { display: flex; align-items: center; gap: 0.6rem; text-decoration: none; }
.pb-brand img { width: 42px; height: auto; }
.pb-brand span { font-family: var(--serif); font-size: 1.5rem; font-weight: 500; letter-spacing: -0.005em; }
.pb-nav { display: flex; gap: 1.4rem; font-size: 0.9rem; }
.pb-nav a, .pb-foot a { color: var(--faint); text-decoration: none; transition: color 0.2s; }
.pb-nav a:hover, .pb-foot a:hover { color: var(--ink); }

.pb-copy { position: absolute; z-index: 2; left: var(--gutter); top: 50%;
  transform: translateY(-50%); width: min(30rem, calc(100% - 2 * var(--gutter)));
  display: flex; flex-direction: column; gap: 1.6rem; }
/* Both beats share one grid cell, so the cell is the taller beat's height and
   the rig under it never moves as they trade places. */
.pb-beats { display: grid; }
.pb-beat { grid-area: 1 / 1; display: flex; flex-direction: column; gap: 1rem;
  transition: opacity 0.7s ease, transform 0.7s cubic-bezier(0.2, 0.8, 0.2, 1); }
.pb-beats[data-beat="0"] .pb-beat-1,
.pb-beats[data-beat="1"] .pb-beat-0 { opacity: 0; transform: translateY(10px); pointer-events: none; }
.pb-lede { margin: 0; font-family: var(--serif); font-size: clamp(1.55rem, 2.6vw, 2.1rem);
  line-height: 1.15; color: var(--dim); }
.pb-hero { margin: 0; font-family: var(--serif); font-weight: 500;
  font-size: clamp(2.4rem, 4.6vw, 3.4rem); line-height: 1.02; letter-spacing: -0.012em; }
.pb-sub { margin: 0; font-size: 1rem; line-height: 1.6; color: var(--dim); max-width: 27rem; }
/* Reserves the open rig's height, so the vertically centred copy above never
   moves when the button opens. The fine print rides under whichever control
   is showing and the unused room sits below it, where it reads as margin. */
.pb-slot { min-height: 8.2rem; display: flex; flex-direction: column; align-items: flex-start; gap: 0.65rem; }
.pb-slot .pb-cta { margin-top: 0; }
/* The rig arrives where the button was, rising out of the slot rather than
   popping in, so the press reads as the button opening up. */
.pb-slot .pb-rig { animation: pb-rig-in 0.45s cubic-bezier(0.2, 0.8, 0.2, 1); }
@keyframes pb-rig-in { from { opacity: 0; transform: translateY(6px); } }
.pb-fine { margin: 0; font-size: 0.8rem; color: var(--faintest); }
.pb-copy .pb-rig { margin-top: 0; }
.pb-confirm { margin: 0; color: var(--ink); font-size: 1rem; overflow-wrap: anywhere; }
.pb-status { margin: 0; color: var(--faint); font-size: 0.9rem; line-height: 1.5; max-width: 26rem; }
.pb-status-error { color: var(--danger); }
.pb-cue { position: absolute; z-index: 2; left: 50%; bottom: 1.4rem; transform: translateX(-50%);
  font-size: 0.72rem; letter-spacing: 0.14em; text-transform: uppercase; color: var(--faintest);
  transition: opacity 0.5s ease; display: flex; flex-direction: column; align-items: center; gap: 0.5rem; }
.pb-cue::after { content: ""; width: 1px; height: 26px;
  background: linear-gradient(var(--faintest), transparent); animation: pb-cue 2.2s ease-in-out infinite; }
@keyframes pb-cue { 0%, 100% { transform: scaleY(0.4); transform-origin: top; } 50% { transform: scaleY(1); transform-origin: top; } }
.pb-stage[data-scrolled] .pb-cue { opacity: 0; }

@media (max-width: 820px) {
  .pb-scrim { background:
    linear-gradient(180deg, rgba(9,13,22,0.95) 0%, rgba(9,13,22,0.7) 42%, rgba(9,13,22,0) 72%),
    linear-gradient(180deg, rgba(9,13,22,0) 78%, rgba(9,13,22,0.9) 100%); }
  .pb-copy { top: 5.2rem; transform: none; gap: 1.25rem; }
  .pb-nav a:not(.pb-keep) { display: none; }
  .pb-cue, .pb-mkrs, .pb-read-l { display: none; }
}

/* BEFORE / AFTER. */
.pb-section { padding: clamp(4.5rem, 10vw, 8rem) var(--gutter); max-width: 84rem; margin: 0 auto; }
.pb-h2 { margin: 0 0 0.9rem; font-family: var(--serif); font-weight: 500;
  font-size: clamp(2rem, 3.8vw, 2.9rem); line-height: 1.05; letter-spacing: -0.01em; }
.pb-h2 em { font-style: normal; color: var(--accent-ink); }
.pb-intro { margin: 0 0 clamp(2rem, 5vw, 3.25rem); color: var(--dim); font-size: 1.05rem;
  line-height: 1.6; max-width: 34rem; }
.pb-pair { display: grid; grid-template-columns: minmax(0, 0.58fr) auto minmax(0, 1.42fr);
  gap: clamp(0.75rem, 1.6vw, 1.25rem); align-items: stretch; }
@media (max-width: 900px) { .pb-pair { grid-template-columns: minmax(0, 1fr); } }
/* THE GATE between the two panels: a lit filament with the squelch sitting on
   it, the same passband blue the scope's filter curve draws in. The before is
   everything upstream of it and the after is what comes out, so the divider is
   the product rather than a gap. Horizontal once the panels stack. */
.pb-gate { position: relative; width: 3.25rem; display: grid; place-items: center;
  margin-top: 2rem; /* level with the windows, below the captions */ }
.pb-gate::before { content: ""; position: absolute; top: 0; bottom: 0; left: 50%; width: 1px;
  background: linear-gradient(transparent, rgba(78, 155, 234, 0.75) 25%, rgba(78, 155, 234, 0.75) 75%, transparent);
  box-shadow: 0 0 14px 1px rgba(78, 155, 234, 0.45); }
.pb-gate span { position: relative; width: 2.6rem; height: 2.6rem; border-radius: 50%; display: grid;
  place-items: center; color: var(--accent-ink); background: #0d1624;
  box-shadow: inset 0 0 0 1px rgba(78, 155, 234, 0.55), 0 0 26px -2px rgba(78, 155, 234, 0.55); }
.pb-gate svg { width: 1.05rem; height: 1.05rem; }
@media (max-width: 900px) {
  .pb-gate { width: auto; height: 3.25rem; margin: 0.25rem 0; }
  .pb-gate::before { top: 50%; bottom: auto; left: 0; right: 0; width: auto; height: 1px;
    background: linear-gradient(90deg, transparent, rgba(78, 155, 234, 0.75) 25%, rgba(78, 155, 234, 0.75) 75%, transparent); }
  .pb-gate svg { transform: rotate(90deg); }
}
.pb-panel { margin: 0; display: flex; flex-direction: column; gap: 0.8rem; min-width: 0; }
.pb-panel figcaption { display: flex; align-items: baseline; gap: 0.6rem; font-size: 0.82rem; color: var(--faint); }
.pb-panel figcaption b { font-size: 0.7rem; letter-spacing: 0.14em; text-transform: uppercase;
  font-weight: 600; color: var(--faintest); }
.pb-panel figcaption b { font-size: 0.78rem; }
.pb-after figcaption b { color: var(--accent); }
.pb-after figcaption { color: var(--dim); }
/* Before is drained: grey, dim, a world with no signal in it. Only its unread
   count keeps its colour, which is the point of it. After is lit: edged in
   passband blue with the glow of the gate it came through. */
.pb-before .pb-window { filter: saturate(0.25) brightness(0.82); background: #0c1017; }
.pb-after .pb-window { border-color: rgba(78, 155, 234, 0.45);
  box-shadow: 0 0 0 1px rgba(78, 155, 234, 0.12), 0 0 60px -12px rgba(78, 155, 234, 0.35),
    0 30px 80px -20px rgba(0, 0, 0, 0.7); }
.pb-window { position: relative; flex: 1; border-radius: 14px; overflow: hidden; min-height: 30rem;
  border: 1px solid var(--hair-strong); background: var(--canvas);
  box-shadow: 0 30px 80px -20px rgba(0, 0, 0, 0.7), inset 0 1px 0 rgba(255, 255, 255, 0.04); }
@media (max-width: 900px) { .pb-before .pb-window { min-height: 18rem; } }

/* The doomscroll. Dimmed and faded at both ends: it is the "before", so it
   should read as a texture you are drowning in, not as something to read. */
.pb-inbox { position: absolute; inset: 0; overflow: hidden;
  -webkit-mask-image: linear-gradient(transparent, #000 14%, #000 80%, transparent);
          mask-image: linear-gradient(transparent, #000 14%, #000 80%, transparent); }
.pb-row { display: flex; align-items: baseline; gap: 0.75rem; padding: 0.6rem 1rem;
  border-bottom: 1px solid var(--hair); white-space: nowrap; font-size: 0.8rem; }
.pb-row-sender { width: 7.5rem; flex-shrink: 0; font-weight: 600; color: #b9c2cd;
  overflow: hidden; text-overflow: ellipsis; }
.pb-row-text { flex: 1; min-width: 0; overflow: hidden; text-overflow: ellipsis; color: var(--faintest); }
.pb-row-text span { color: #9aa5b3; }
.pb-row-time { flex-shrink: 0; font-size: 0.7rem; color: var(--faintest); }
.pb-unread { margin-left: auto; font-size: 0.72rem; font-weight: 600;
  padding: 0.2rem 0.55rem; border-radius: 99px; color: #fff; background: #c7433a;
  font-variant-numeric: tabular-nums; }

/* THE AFTER: a fake screenshot of the sitrep, built in HTML rather than shipped
   as a PNG so it stays sharp at every size and every word in it is real text.

   Measured off the real app (the practice board, shot at 1440pt) and written
   in the app's own POINTS: every "Npt" below is rewritten to a multiple of
   --pt, which is the window's width over the width the mock is laid out at (1040pt, about the app's smallest window).
   So the mock is the app at one fixed layout, scaled as a picture would be,
   rather than a web page that reflows into something the app never draws.
   Colours are sampled from the same shot. */
.pb-after .pb-window { container-type: inline-size; min-height: 0; background: #262c35; }
.pb-app { --pt: calc(100cqw / 1040); display: grid; grid-template-columns: 58pt minmax(0, 1fr);
  grid-template-rows: 40pt auto; font-size: 12pt; color: var(--ink); line-height: 1.25; }
.pb-app-bar { grid-column: 1 / -1; display: flex; align-items: center; gap: 14pt; padding: 0 18pt 0 16pt; }
.pb-lights { display: flex; gap: 8pt; }
.pb-lights i { width: 12pt; height: 12pt; border-radius: 50%; background: #ec6a5e; }
.pb-lights i:nth-child(2) { background: #f4bf4f; } .pb-lights i:nth-child(3) { background: #61c554; }
.pb-app-title { display: flex; align-items: baseline; gap: 8pt; margin-left: 12pt; }
.pb-app-title b { font-family: var(--serif); font-weight: 500; font-size: 19pt; }
.pb-app-title small { font-size: 10pt; font-weight: 500; letter-spacing: 0.06em; color: var(--faintest); }
.pb-app-tools { margin-left: auto; display: flex; align-items: center; gap: 12pt; font-size: 10pt;
  font-weight: 500; color: var(--faint); }
.pb-app-tools svg { width: 10pt; height: 10pt; vertical-align: -1pt; margin-right: 3pt; }
.pb-need { display: inline-flex; align-items: center; gap: 5pt; color: var(--danger); font-size: 11pt;
  font-weight: 500; padding: 3pt 9pt; border-radius: 99pt; background: rgba(255, 122, 104, 0.14);
  box-shadow: inset 0 0 0 0.75pt rgba(255, 122, 104, 0.45); }
.pb-need::before { content: ""; width: 5pt; height: 5pt; border-radius: 50%; background: var(--danger); }
.pb-side { background: #2c323f; border-right: 1px solid #363b48; border-top-right-radius: 10pt;
  display: flex; flex-direction: column; align-items: center; gap: 13pt; padding: 12pt 0 16pt; }
.pb-side span { width: 36pt; height: 30pt; display: grid; place-items: center; border-radius: 8pt; color: #9aa6b6; }
.pb-side span.on { background: rgba(78, 155, 234, 0.2); color: var(--accent); }
.pb-side svg { width: 18pt; height: 18pt; }
.pb-side .pb-side-gap { flex: 1; }
.pb-side .pb-me { width: 20pt; height: 20pt; border-radius: 50%; background: #24384F; color: #A9CBF0;
  font-size: 10pt; font-weight: 600; }
.pb-page { padding: 6pt 22pt 22pt 30pt; display: grid; grid-template-columns: minmax(0, 1fr) 252pt;
  column-gap: 14pt; align-items: start; }
.pb-dash-hero { grid-column: 1 / -1; margin-bottom: 16pt; }
.pb-dash-hero small { display: block; font-size: 10pt; font-weight: 500; letter-spacing: 0.12em;
  text-transform: uppercase; color: var(--accent); margin-bottom: 2pt; }
.pb-dash-hero b { font-family: var(--serif); font-weight: 500; font-size: 36pt; letter-spacing: -0.01em; line-height: 1.08; }
.pb-col { display: flex; flex-direction: column; gap: 14pt; min-width: 0; }
.pb-zone { border-radius: 16pt; padding: 12pt 12pt 10pt; background: #1f2c3a;
  box-shadow: inset 0 0 0 0.75pt #2f4459; }
.pb-zone-h { display: flex; align-items: center; gap: 7pt; padding: 0 4pt 7pt; font-size: 13pt; font-weight: 600; }
.pb-zone-h svg { width: 14pt; height: 14pt; color: var(--zone, var(--accent)); flex: none; }
.pb-zone-h em { font-style: normal; font-size: 10pt; font-weight: 500; color: var(--faint);
  background: rgba(233, 238, 245, 0.08); border-radius: 99pt; padding: 1pt 6pt; }
.pb-zone-h small { font-size: 10pt; font-weight: 500; color: var(--faintest); margin-left: 2pt; }
.pb-eye { position: relative; display: flex; align-items: center; gap: 9pt; padding: 7pt 10pt; border-radius: 8pt;
  white-space: nowrap; }
.pb-eye.cursor { background: rgba(78, 155, 234, 0.1); }
.pb-eye.overdue::before { content: ""; position: absolute; left: 0; top: 5pt; bottom: 5pt; width: 2pt;
  border-radius: 1pt; background: var(--danger); }
.pb-avatar { width: 22pt; height: 22pt; border-radius: 50%; flex: none; display: grid; place-items: center;
  font-size: 9pt; font-weight: 600; }
.pb-eye b { font-weight: 500; flex: none; }
.pb-eye span.line { color: var(--dim); overflow: hidden; text-overflow: ellipsis; flex: 1; min-width: 0; }
.pb-chip { flex: none; font-size: 10pt; font-weight: 500; padding: 2.5pt 7pt; border-radius: 99pt; color: var(--c);
  box-shadow: inset 0 0 0 0.75pt color-mix(in srgb, var(--c) 30%, transparent); }
.pb-chip.filled { background: color-mix(in srgb, var(--c) 16%, transparent); }
.pb-reading { display: grid; grid-template-columns: repeat(3, minmax(0, 1fr)); gap: 8pt; }
.pb-read { display: flex; gap: 10pt; padding: 8pt; border-radius: 12pt; background: #283647; min-width: 0; }
.pb-logo { width: 46pt; height: 46pt; border-radius: 8pt; flex: none; display: grid; place-items: center;
  font-weight: 700; font-size: 17pt; }
.pb-read-text { min-width: 0; flex: 1; display: flex; flex-direction: column; gap: 3pt; }
.pb-read-top { display: flex; justify-content: space-between; gap: 6pt; }
.pb-read-top b { font-weight: 600; white-space: nowrap; overflow: hidden; text-overflow: ellipsis; }
.pb-read-top span { font-size: 10pt; color: var(--faintest); white-space: nowrap; }
.pb-read p { margin: 0; font-size: 10.5pt; line-height: 1.3; color: var(--faint);
  display: -webkit-box; -webkit-line-clamp: 2; -webkit-box-orient: vertical; overflow: hidden; }
/* The records rail: one TINTED card per zone, the zone's own hue washed into
   its ground and its border, which is the single biggest thing that makes the
   sitrep read as the sitrep. */
.pb-rail { display: flex; flex-direction: column; gap: 12pt; min-width: 0; }
.pb-rec { border-radius: 16pt; padding: 11pt 11pt 9pt; background: var(--bg-c); box-shadow: inset 0 0 0 0.75pt var(--edge); }
.pb-rec .pb-zone-h { padding: 0 3pt 5pt; }
.pb-rec-cal { --bg-c: #253341; --edge: #3b5673; --zone: var(--accent); }
.pb-rec-ship { --bg-c: #35332d; --edge: #6a5a3e; --zone: var(--warn); }
.pb-rec-bill { --bg-c: #2a3540; --edge: #3f566d; --zone: var(--accent); }
.pb-rec-rcpt { --bg-c: #233635; --edge: #2f5a49; --zone: var(--positive); }
.pb-rec-row { display: flex; justify-content: space-between; align-items: center; gap: 8pt; padding: 4.5pt 3pt;
  font-size: 11.5pt; white-space: nowrap; }
.pb-rec-row b { font-weight: 400; color: #cdd5df; overflow: hidden; text-overflow: ellipsis; }
.pb-rec-row span { font-size: 10pt; color: var(--faintest); font-variant-numeric: tabular-nums; }
.pb-rec-row .pb-chip { --c: var(--faint); }
.pb-rec-rcpt .pb-rec-row span { font-size: 11.5pt; font-weight: 600; color: var(--positive); }
.pb-ship { background: #3f3d38; border-radius: 10pt; padding: 7pt 8pt; margin-bottom: 6pt;
  display: flex; flex-direction: column; gap: 6pt; }
.pb-ship-top { display: flex; align-items: center; gap: 7pt; white-space: nowrap; }
.pb-ship-top i { width: 14pt; height: 14pt; border-radius: 3pt; flex: none; background: #5a4631;
  display: grid; place-items: center; font-style: normal; font-size: 8pt; font-weight: 700; color: #f4c47a; }
.pb-ship-top b { font-weight: 600; flex: 1; min-width: 0; overflow: hidden; text-overflow: ellipsis; }
.pb-ship .pb-chip.eta { --c: var(--faint); align-self: flex-start; }

/* Phones: the same app, laid out the way its own narrow window does it. The
   rail stops being a pinned column and stacks under the work surface, and the
   side rail goes, so the mock is scaled for 560pt instead of 1120pt. */
@container (max-width: 640px) {
  .pb-app { --pt: calc(100cqw / 520); grid-template-columns: minmax(0, 1fr); }
  .pb-side, .pb-app-tools .pb-retriage { display: none; }
  .pb-page { grid-template-columns: minmax(0, 1fr); padding: 6pt 16pt 16pt; row-gap: 12pt; }
  .pb-reading { grid-template-columns: repeat(2, minmax(0, 1fr)); }
  .pb-reading .pb-read:nth-child(3) { display: none; }
  .pb-rail { display: grid; grid-template-columns: repeat(2, minmax(0, 1fr)); gap: 10pt; }
  .pb-rec-cal { display: none; }
}

/* THE TRUST ROW. Three facts, no icons: the claims are specific enough to
   carry themselves, and an icon beside each is how a list becomes slop. */
.pb-facts { display: grid; grid-template-columns: repeat(3, minmax(0, 1fr)); gap: clamp(1.5rem, 4vw, 3.5rem);
  border-top: 1px solid var(--hair); padding-top: clamp(2rem, 4vw, 3rem); }
@media (max-width: 760px) { .pb-facts { grid-template-columns: minmax(0, 1fr); } }
.pb-fact h3 { margin: 0 0 0.55rem; font-size: 0.98rem; font-weight: 600; }
.pb-fact p { margin: 0; color: var(--dim); font-size: 0.93rem; line-height: 1.6; }
.pb-fact a { color: var(--accent-ink); text-decoration: none; }
.pb-fact a:hover { text-decoration: underline; }

.pb-close { text-align: center; display: flex; flex-direction: column; align-items: center; gap: 1.4rem; }
.pb-close .pb-h2 { margin: 0; }
/* No meter behind this one, so no clearance for it under the label. */
.pb-close .pb-cta { padding: 0.9rem 1.9rem; }
.pb-foot { display: flex; flex-wrap: wrap; justify-content: space-between; gap: 1rem;
  padding: 1.75rem var(--gutter) 2.25rem; border-top: 1px solid var(--hair); font-size: 0.85rem; color: var(--faintest); }
.pb-foot nav { display: flex; flex-wrap: wrap; gap: 1.25rem; }

@media (prefers-reduced-motion: reduce) {
  .pb-beat, .pb-cue { transition-duration: 0.01ms; }
  .pb-slot .pb-rig { animation: none; }
  .pb-cue::after { animation: none; }
}
`;

// The mock's "Npt" lengths are the app's points: rewritten to multiples of
// --pt, which scales the whole mock with its window (see THE AFTER above).
const PAGE_CSS_RESOLVED = PAGE_CSS.replace(
  /(\d*\.?\d+)pt\b/g,
  "calc($1 * var(--pt))",
);

function FakeInbox() {
  // One randomized batch of rows, rendered twice so the scroll can wrap
  // seamlessly: when the offset passes one copy's height it resets mod that
  // height and the second copy is pixel-identical to where the first began.
  // The mail that mattered is dealt in every dozen rows or so, unmarked.
  const rows = useMemo(
    () =>
      Array.from({ length: 60 }, (_, i) => {
        const [sender, subject, snippet] =
          i % 13 === 5
            ? BURIED[Math.floor(i / 13) % BURIED.length]
            : FAKE_EMAILS[Math.floor(Math.random() * FAKE_EMAILS.length)];
        const h = Math.floor(Math.random() * 12) + 1;
        const m = String(Math.floor(Math.random() * 60)).padStart(2, "0");
        return { sender, subject, snippet, time: `${h}:${m}` };
      }),
    [],
  );

  const scrollRef = useRef<HTMLDivElement>(null);

  // Fake doomscroll: a flick of random distance and speed, a random pause,
  // repeat forever. rAF drives each flick; timeouts space them out. It only
  // runs while the panel is on screen.
  useEffect(() => {
    if (matchMedia("(prefers-reduced-motion: reduce)").matches) return;
    const el = scrollRef.current;
    if (!el) return;
    let offset = 0;
    let raf = 0;
    let timer: ReturnType<typeof setTimeout> | undefined;
    let running = false;
    const easeOut = (t: number) => 1 - Math.pow(1 - t, 3);

    const flick = () => {
      if (!running) return;
      const distance = 40 + Math.random() * 280;
      const duration = 800 + Math.random() * 1400;
      const from = offset;
      const start = performance.now();
      const frame = (now: number) => {
        if (!running) return;
        const t = Math.min(1, (now - start) / duration);
        offset = from + distance * easeOut(t);
        const wrap = el.scrollHeight / 2 || 1;
        el.style.transform = `translateY(-${offset % wrap}px)`;
        if (t < 1) raf = requestAnimationFrame(frame);
        else timer = setTimeout(flick, 150 + Math.random() * 1100);
      };
      raf = requestAnimationFrame(frame);
    };

    const io = new IntersectionObserver(([entry]) => {
      if (entry.isIntersecting && !running) {
        running = true;
        timer = setTimeout(flick, 300);
      } else if (!entry.isIntersecting && running) {
        running = false;
        cancelAnimationFrame(raf);
        clearTimeout(timer);
      }
    });
    io.observe(el.parentElement!);
    return () => {
      running = false;
      io.disconnect();
      cancelAnimationFrame(raf);
      clearTimeout(timer);
    };
  }, []);

  return (
    <div className="pb-inbox" aria-hidden="true">
      <div ref={scrollRef} style={{ willChange: "transform" }}>
        {[...rows, ...rows].map(({ sender, subject, snippet, time }, i) => (
          <div key={i} className="pb-row">
            <span className="pb-row-sender">{sender}</span>
            <span className="pb-row-text">
              <span>{subject}</span> {snippet}
            </span>
            <span className="pb-row-time">{time}</span>
          </div>
        ))}
      </div>
    </div>
  );
}

// Passband blue, the app's own dark-mode accent (Palette.accent, 4E9BEA).
// The rig is the one lit instrument on a page that is otherwise the scene.
const ACCENT = "78, 155, 234";

// The passband's half-width when the filter is fully open. Everything narrower
// is this times the opening, which is what keeps the hump one shape instead of
// a stretching blob. Module scope because the pointer handler sizes its travel
// limit from it and the draw loop sizes the curve from it.
const FULL_W = 0.19;

const CTA_CSS = `
.pb-cta {
  position: relative;
  isolation: isolate;
  display: inline-flex;
  align-items: center;
  justify-content: center;
  gap: 0.6rem;
  margin-top: 0.65rem;
  padding: 0.9rem 1.9rem 1.3rem;
  border-radius: 0.9rem;
  overflow: hidden;
  text-decoration: none;
  font-family: inherit;
  font-size: 0.95rem;
  font-weight: 600;
  letter-spacing: 0.015em;
  cursor: pointer;
  appearance: none;
  color: #dfe8f3;
  background: linear-gradient(180deg, #152032, #0c121c);
  border: 1px solid rgba(${ACCENT}, 0.22);
  box-shadow:
    inset 0 1px 0 rgba(255, 255, 255, 0.07),
    0 12px 30px rgba(0, 0, 0, 0.5);
  transition:
    transform 0.4s cubic-bezier(0.2, 0.8, 0.2, 1),
    border-color 0.4s ease,
    box-shadow 0.4s ease,
    color 0.4s ease;
}
.pb-cta:hover,
.pb-cta:focus-visible {
  color: #f4f8fd;
  transform: translateY(-2px);
  border-color: rgba(${ACCENT}, 0.62);
  box-shadow:
    inset 0 1px 0 rgba(255, 255, 255, 0.14),
    0 0 34px -6px rgba(${ACCENT}, 0.45),
    0 16px 38px rgba(0, 0, 0, 0.55);
}
.pb-cta:active { transform: translateY(0) scale(0.995); }
.pb-cta:focus-visible {
  outline: 2px solid rgba(${ACCENT}, 0.75);
  outline-offset: 3px;
}
/* Machined top edge: a filament that comes up with the rest of the hardware. */
.pb-cta::before {
  content: "";
  position: absolute;
  inset: 0 0 auto;
  height: 1px;
  z-index: 2;
  opacity: 0.45;
  background: linear-gradient(90deg, transparent, rgba(${ACCENT}, 0.8), transparent);
  transition: opacity 0.4s ease;
}
.pb-cta:hover::before,
.pb-cta:focus-visible::before { opacity: 1; }
.pb-cta-meter {
  position: absolute;
  inset: 0;
  z-index: 0;
  width: 100%;
  height: 100%;
  /* Ground, never a target. Inside the rig it lies across the address field,
     and a canvas that swallowed the click would leave the field unfocusable. */
  pointer-events: none;
}
.pb-cta-arrow,
.pb-cta-label { position: relative; z-index: 1; }
.pb-cta-arrow { transition: transform 0.4s cubic-bezier(0.2, 0.8, 0.2, 1); }
.pb-cta:hover .pb-cta-arrow,
.pb-cta:focus-visible .pb-cta-arrow { transform: translateX(3px); }
/* THE RIG: the email field and the join button as ONE piece of machined
   hardware rather than a form on a card.
   The card that used to be here was a translucent rounded rectangle with a
   hairline white border, which is the single most templated component on the
   web and belonged to no part of this page. The page has exactly one material
   for controls: a dark machined ground edged in passband blue. So the
   waitlist is built out of THAT, with slots cut in it. The rig owns the material and the enclosure; the button inside it
   keeps only its meter and its label. */
.pb-rig {
  position: relative;
  isolation: isolate;
  /* A COLUMN, because there are two things to give now and three controls will
     not fit across 28rem: at that width the address slot ends up too narrow to
     show a whole address, and on a phone it clips mid-domain. So the rig
     grows downward instead of squeezing sideways, and stays ONE piece of
     hardware with two slots cut in it rather than becoming two controls. */
  display: flex;
  flex-direction: column;
  width: min(28rem, 100%);
  margin-top: 0.65rem;
  border-radius: 0.9rem;
  overflow: hidden;
  background: linear-gradient(180deg, #152032, #0c121c);
  border: 1px solid rgba(${ACCENT}, 0.22);
  box-shadow:
    inset 0 1px 0 rgba(255, 255, 255, 0.07),
    0 12px 30px rgba(0, 0, 0, 0.5);
  transition: border-color 0.4s ease, box-shadow 0.4s ease;
}
/* One lit state for the whole instrument, driven by the field inside it:
   focusing the input is the same event as arming the button, so lighting only
   the half under the cursor would say the two are separate things. */
.pb-rig:hover,
.pb-rig:focus-within {
  border-color: rgba(${ACCENT}, 0.62);
  box-shadow:
    inset 0 1px 0 rgba(255, 255, 255, 0.14),
    0 0 34px -6px rgba(${ACCENT}, 0.45),
    0 16px 38px rgba(0, 0, 0, 0.55);
}
/* The same machined top edge the standalone button wears. */
.pb-rig::before {
  content: "";
  position: absolute;
  inset: 0 0 auto;
  height: 1px;
  z-index: 2;
  opacity: 0.45;
  background: linear-gradient(90deg, transparent, rgba(${ACCENT}, 0.8), transparent);
  transition: opacity 0.4s ease;
}
.pb-rig:hover::before,
.pb-rig:focus-within::before { opacity: 1; }
/* The bottom slot's row. It carries the meter, so the instrument's floor stays
   exactly the height it was rather than growing with the rig: the bars are
   drawn behind the address and the button, which is the half where the action
   is, and the name sits above the whole display. */
.pb-rig-row {
  position: relative;
  display: flex;
  align-items: stretch;
}
/* The slot. No border and no ground of its own: it is an opening in the rig,
   not a control sitting on one. Its padding matches the button's exactly, so
   the address and the label sit on the same optical line. */
.pb-rig-field {
  position: relative;
  z-index: 1;
  flex: 1 1 auto;
  min-width: 0;
  padding: 0.9rem 1.1rem 1.3rem;
  border: 0;
  outline: none;
  background: none;
  color: #E9EEF5;
  font-family: inherit;
  font-size: 0.95rem;
  line-height: 1.2;
}
.pb-rig-field::placeholder { color: #62707F; }
.pb-rig-field:disabled { color: #808E9F; }
/* WHICH SLOT IS LIVE. The rig lights as one instrument on :focus-within,
   which said everything worth saying while there was one opening in it and
   nothing at all once there were two: tabbing between them changed no pixel
   but the caret. A lit edge down the active slot is the smallest thing that
   answers it in the instrument's own language, and it is :focus rather than
   :focus-visible because the question ("which one am I typing into") is the
   same however the slot was reached.

   outline: none above is why this is a shadow: the outline is the affordance
   this design gave up, and an inset edge is the one that belongs on a slot cut
   into a face. */
.pb-rig-field:focus { box-shadow: inset 2px 0 0 rgba(${ACCENT}, 0.7); }
/* Chrome recognises name beside email as an address profile and paints its
   own opaque ground into both slots, which on a rig whose whole premise is that
   the slots have no ground of their own is the one thing that breaks the
   material. The inset shadow is the documented way to overrule it. */
.pb-rig-field:-webkit-autofill,
.pb-rig-field:-webkit-autofill:hover,
.pb-rig-field:-webkit-autofill:focus {
  -webkit-text-fill-color: #E9EEF5;
  caret-color: #E9EEF5;
  box-shadow: inset 0 0 0 100vw #101826;
  transition: background-color 9999s;
}
.pb-rig-field:-webkit-autofill:focus {
  box-shadow: inset 0 0 0 100vw #101826, inset 2px 0 0 rgba(${ACCENT}, 0.7);
}
/* The name slot. Divided off from the row below by a hairline, so the two
   openings read as machined out of one face rather than as one box with two
   bits of text floating in it. Its own padding, because the address slot's
   asymmetric bottom is clearance for the meter and there is no meter behind
   this one. */
.pb-rig-name {
  flex: none;
  padding: 0.85rem 1.1rem 0.8rem;
  /* Brighter than the button's own divider, and it has to be: that one runs
     down the lit half of the instrument, with the meter's glow behind it,
     while this one crosses dark ground where the same 0.18 vanishes. */
  border-bottom: 1px solid rgba(${ACCENT}, 0.3);
}
/* The button, once the rig owns the material: no ground, no shell, no lift.
   What is left of it is the half that lights up, divided off by one hairline.
   The lift is dropped deliberately, because a button that rises out of the bar
   it is set into reads as a part coming loose. */
.pb-cta-in-rig {
  flex: none;
  margin-top: 0;
  border: 0;
  border-left: 1px solid rgba(${ACCENT}, 0.18);
  border-radius: 0;
  background: none;
  box-shadow: none;
}
.pb-cta-in-rig::before { display: none; }
.pb-cta-in-rig:hover,
.pb-cta-in-rig:focus-visible {
  transform: none;
  box-shadow: none;
  border-color: rgba(${ACCENT}, 0.35);
}
/* Drawn inside, because the rig clips anything outside it. */
.pb-cta-in-rig:focus-visible { outline-offset: -3px; }
/* In flight. The meter keeps running (the request is the thing being waited
   on) but the hardware stops answering the pointer. */
.pb-cta[disabled] { cursor: progress; color: #808E9F; }
.pb-cta[disabled]:hover,
.pb-cta[disabled]:hover .pb-cta-arrow { transform: none; }
.pb-cta[disabled]:hover { border-color: rgba(${ACCENT}, 0.22); box-shadow:
  inset 0 1px 0 rgba(255, 255, 255, 0.07), 0 12px 30px rgba(0, 0, 0, 0.5); }
.pb-cta-in-rig[disabled]:hover { box-shadow: none; border-color: rgba(${ACCENT}, 0.18); }
@media (prefers-reduced-motion: reduce) {
  .pb-cta,
  .pb-cta-arrow { transition-duration: 0.01ms; }
  .pb-cta:hover,
  .pb-cta:focus-visible,
  .pb-cta:hover .pb-cta-arrow,
  .pb-cta:focus-visible .pb-cta-arrow { transform: none; }
}
`;

// The one piece of hardware on the page: the meter behind the waitlist rig. At
// rest it shows a noise floor; on hover the filter closes and only the
// passband survives, lit in passband blue. It is the scene behind it, shrunk to
// the size of a button. The animation is the product's own metaphor, which is the price
// of putting motion here at all.
//
// A HOOK RATHER THAN A COMPONENT because the pointer handlers have to sit on
// the button, not on the canvas inside it: the meter is ground beneath a label,
// and the geometry it reads is the button's own box. So the hook hands back
// both halves, the caller spreads the handlers onto whatever element it is
// building, and the two pages share one meter rather than growing two that
// drift apart.
function useMeter() {
  const canvasRef = useRef<HTMLCanvasElement>(null);
  // Hover lives in a ref, not state: the rAF loop reads it every frame and CSS
  // already owns the chrome, so a re-render would buy nothing.
  const hovered = useRef(false);
  // Where the passband is tuned to and how far it is opened, both 0..1 and both
  // driven by the pointer: x slides the band along the button, y opens it up.
  // Same reasoning as above: a ref, read per frame, never re-rendering.
  const tuned = useRef({ x: 0.5, lift: 1 });

  useEffect(() => {
    const canvas = canvasRef.current;
    const ctx = canvas?.getContext("2d");
    if (!canvas || !ctx) return;
    const reduce = matchMedia("(prefers-reduced-motion: reduce)").matches;

    // Each bar drifts on its own beat so the floor shimmers rather than
    // marching in step. Sized to the widest bar count the button can ask for.
    const POOL = 96;
    const phase = Array.from({ length: POOL }, () => Math.random() * Math.PI * 2);
    const speed = Array.from({ length: POOL }, () => 0.7 + Math.random() * 1.9);

    // The filter's frequency response: flat across the top, steep skirts either
    // side. This curve is the passband the product is named for. `c` is where it
    // is tuned to and `w` how wide it is opened.
    const band = (x: number, c: number, w: number) =>
      Math.exp(-Math.pow(Math.abs(x - c) / w, 4));

    let width = 0;
    let height = 0;
    let gate = 0; // 0 = wide open (noise), 1 = filtered down to the passband
    // The drawn centre and opening, chasing `tuned`. Eased rather than assigned
    // so a fast cursor pulls the band along instead of teleporting it, and so
    // leaving the button glides it home rather than snapping.
    let centre = 0.5;
    // Named `aperture`, not `open`: the bar loop below already has its own
    // local `open` for a bar's unfiltered height, and reading an outer `open`
    // earlier in that same block lands in the temporal dead zone and throws
    // on every frame. Syntax checks do not catch it; the canvas just stays
    // blank.
    let aperture = 1;

    const render = (t: number) => {
      ctx.clearRect(0, 0, width, height);
      // Width tracks height, so raising the cursor grows the hump rather than
      // stretching it: the skirts spread at the same rate the peak climbs and
      // the silhouette stays the same shape at every size.
      const bandW = FULL_W * aperture;
      const bars = Math.max(16, Math.min(POOL, Math.round(width / 5.5)));
      const step = width / bars;
      const barW = Math.max(1.5, step - 2);
      const maxH = height * 0.44;

      // Trace the response curve only once the filter is closing, so it reads
      // as the cause of the collapse rather than as decoration.
      if (gate > 0.01) {
        const top = (x: number) =>
          height - (0.06 + 0.94 * aperture * band(x / width, centre, bandW)) * maxH;
        ctx.beginPath();
        ctx.moveTo(0, height);
        for (let x = 0; x <= width; x += 2) ctx.lineTo(x, top(x));
        ctx.lineTo(width, height);
        const fill = ctx.createLinearGradient(0, height - maxH, 0, height);
        fill.addColorStop(0, `rgba(${ACCENT}, ${0.16 * gate})`);
        fill.addColorStop(1, `rgba(${ACCENT}, 0)`);
        ctx.fillStyle = fill;
        ctx.fill();

        ctx.beginPath();
        ctx.moveTo(0, top(0));
        for (let x = 2; x <= width; x += 2) ctx.lineTo(x, top(x));
        ctx.strokeStyle = `rgba(${ACCENT}, ${0.45 * gate})`;
        ctx.lineWidth = 1;
        ctx.stroke();
      }

      for (let i = 0; i < bars; i++) {
        const x = bars > 1 ? i / (bars - 1) : 0.5;
        const response = aperture * band(x, centre, bandW);
        // Two incommensurate beats per bar: busy, but never repeating.
        const noise =
          0.5 +
          0.5 *
            Math.sin(t * speed[i] + phase[i]) *
            Math.cos(t * speed[i] * 0.61 + phase[i] * 1.7);
        const open = 0.12 + 0.3 * noise;
        const filtered = 0.04 + 0.96 * response * (0.55 + 0.45 * noise);
        const barH = Math.max(1, (open * (1 - gate) + filtered * gate) * maxH);

        // Warmth is gated response: only bars the filter passes light up.
        const warm = gate * response;
        const mix = (cold: number, hot: number) =>
          Math.round(cold + (hot - cold) * warm);
        ctx.fillStyle = `rgba(${mix(128, 130)}, ${mix(142, 186)}, ${mix(159, 245)}, ${0.4 + 0.55 * warm})`;
        ctx.shadowBlur = warm > 0.25 ? 10 * warm : 0;
        ctx.shadowColor = `rgba(${ACCENT}, ${0.7 * warm})`;
        const bx = i * step + (step - barW) / 2;
        ctx.beginPath();
        ctx.roundRect(bx, height - barH, barW, barH, barW / 2);
        ctx.fill();
      }
      ctx.shadowBlur = 0;
    };

    const resize = () => {
      const dpr = Math.min(2, window.devicePixelRatio || 1);
      const rect = canvas.getBoundingClientRect();
      width = rect.width;
      height = rect.height;
      canvas.width = Math.round(width * dpr);
      canvas.height = Math.round(height * dpr);
      ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
      // Resizing clears the bitmap, and with no loop running nothing would
      // repaint it.
      if (reduce) render(0);
    };

    let raf = 0;
    let last = 0;
    const loop = (now: number) => {
      const dt = last ? Math.min(0.05, (now - last) / 1000) : 0;
      last = now;
      // Exponential approach, so the ease is the same at 60Hz and 120Hz.
      gate += ((hovered.current ? 1 : 0) - gate) * (1 - Math.exp(-dt * 9));
      // Tuning tracks faster than the filter opens: the band should feel
      // attached to the cursor, while the collapse into it stays a beat behind.
      centre += (tuned.current.x - centre) * (1 - Math.exp(-dt * 16));
      aperture += (tuned.current.lift - aperture) * (1 - Math.exp(-dt * 16));
      render(now / 1000);
      raf = requestAnimationFrame(loop);
    };

    resize();
    const observer = new ResizeObserver(resize);
    observer.observe(canvas);
    // Reduced motion still gets the meter, just held on a single frame.
    if (reduce) render(0);
    else raf = requestAnimationFrame(loop);

    return () => {
      cancelAnimationFrame(raf);
      observer.disconnect();
    };
  }, []);

  return {
    // Everything that goes INSIDE the button: the stylesheet, and the canvas
    // the loop above draws on.
    chrome: (
      <>
        {/* href + precedence is React 19's hoist path: the rules land in <head>
            and dedupe by href instead of sitting loose in the body, which is
            also what lets both buttons render this without shipping it twice. */}
        <style href="pb-cta" precedence="default">
          {CTA_CSS}
        </style>
        <canvas ref={canvasRef} className="pb-cta-meter" aria-hidden="true" />
      </>
    ),
    // Everything that goes ON it.
    handlers: {
      onPointerEnter: () => {
        hovered.current = true;
      },
      onPointerMove: (event: PointerEvent<HTMLElement>) => {
        // THE CANVAS'S BOX, NOT THE HANDLER'S. They were the same element's
        // box until the rig grew a second slot above the meter: the handlers
        // stay on the whole instrument (so hovering anywhere tunes it, which is
        // the design) while the canvas covers only the bottom row, and reading
        // the outer box spent half the vertical travel over a slot with no
        // meter behind it — the filter could never open fully anywhere you
        // could actually see it. On the standalone button the two boxes are
        // still the same rectangle, so nothing changes there.
        const box = (canvasRef.current ?? event.currentTarget).getBoundingClientRect();
        const x = (event.clientX - box.left) / (box.width || 1);
        // Screen y grows downward and the hump grows upward, so invert: the
        // top of the button is the filter wide open.
        const lift = 1 - (event.clientY - box.top) / (box.height || 1);
        // Never fully shut: at zero the hump has no height and no width, so
        // the bars all die and the button looks broken rather than tuned. The
        // top of the travel goes past 1, which is the meter's nominal full
        // height, so the peak reaches up behind the label instead of stopping
        // politely beneath it.
        const aperture = 0.38 + 0.8 * Math.min(1, Math.max(0, lift));
        // Clamped by the skirts' real width, not a fixed margin: a wide hump
        // needs more room to keep both shoulders on the button than a narrow
        // one, so the travel opens up exactly as the filter closes down. The
        // quartic is down to a percent of peak by 1.5 half-widths, so 0.72 is
        // where the shoulder has visually landed.
        const edge = 0.72 * FULL_W * aperture;
        tuned.current = {
          x: Math.min(1 - edge, Math.max(edge, x)),
          lift: aperture,
        };
      },
      onPointerLeave: () => {
        hovered.current = false;
        // Home, so the next hover starts centred and open rather than
        // wherever the last one happened to end.
        tuned.current = { x: 0.5, lift: 1 };
      },
      // Keyboard focus has no cursor to follow, so it gets the centred band.
      onFocus: () => {
        hovered.current = true;
      },
      onBlur: () => {
        hovered.current = false;
      },
    },
  };
}

// The arrow the buttons wear. It points the way the press goes. Label first,
// arrow second, so the two read left to right in the order they happen.
function Arrow() {
  return (
    <svg
      className="pb-cta-arrow"
      width="15"
      height="14"
      viewBox="0 0 15 14"
      fill="none"
      stroke="currentColor"
      strokeWidth="1.6"
      strokeLinecap="round"
      strokeLinejoin="round"
      aria-hidden="true"
    >
      <path d="M1.4 7h11.2m0 0L9.3 3.7M12.6 7 9.3 10.3" />
    </svg>
  );
}

// THE HOMEPAGE'S ONE ACTION. It used to be the download, and the client is not
// a thing worth holding before there is a mailbox behind it: the door here is
// the list, and the download waits at the end of the invite flow, where it is
// the next thing somebody actually needs.
//
// STILL AN ANCHOR with a real `href`, even though the click is handled: it is a
// link to a URL that exists, so cmd-click, middle-click, and "copy link" all
// have to keep meaning what they mean.
function SubmitButton({ busy }: { busy: boolean }) {
  return (
    <button className="pb-cta pb-cta-in-rig" type="submit" disabled={busy}>
      <span className="pb-cta-label">{busy ? "sending" : "join"}</span>
      {!busy && <Arrow />}
    </button>
  );
}

// The control plane answers 200 for a fresh address and for one already on the
// list, so this page can never become a membership oracle.
const WAITLIST_URL = "https://signup.passband.app/waitlist";

// The path the waitlist state answers to. A real URL, deep-linkable and
// shareable, even though reaching it from the button never loads a document.
const WAITLIST_PATH = "/waitlist";

// THE HERO'S ONE ACTION, until it is pressed; then the rig takes its slot.
//
// STILL AN ANCHOR with a real `href`, even though the click is handled: it is a
// link to a URL that exists, so cmd-click, middle-click, and "copy link" all
// have to keep meaning what they mean.
function JoinButton({
  onClick,
}: {
  onClick: (event: MouseEvent<HTMLAnchorElement>) => void;
}) {
  const { chrome, handlers } = useMeter();
  return (
    <a className="pb-cta" href={WAITLIST_PATH} onClick={onClick} {...handlers}>
      {chrome}
      <span className="pb-cta-label">join the waitlist</span>
      <Arrow />
    </a>
  );
}

// The waitlist rig, in the slot the button held. The beats crossfade above it
// and it never moves, so a half-typed address stays exactly where it was while
// the scope resolves behind it.
function Waitlist() {
  const { chrome, handlers } = useMeter();
  const nameRef = useRef<HTMLInputElement>(null);
  // The button that opened this is gone from under the cursor, so the first
  // field takes the focus it left behind: press join, start typing. Not
  // autoFocus: that scrolls the field into view, which from the closing
  // button would cut the smooth scroll back up short.
  useEffect(() => nameRef.current?.focus({ preventScroll: true }), []);
  const [name, setName] = useState("");
  const [email, setEmail] = useState("");
  const [state, setState] = useState<"idle" | "busy" | "done" | "error">("idle");

  const submit = async (event: FormEvent) => {
    event.preventDefault();
    if (state === "busy") return;
    setState("busy");
    try {
      const res = await fetch(WAITLIST_URL, {
        method: "POST",
        // urlencoded keeps this a CORS simple request: no preflight round trip.
        headers: { "Content-Type": "application/x-www-form-urlencoded" },
        // The name rides along beside the address. The control plane treats it
        // as optional, so an older cached copy of this bundle posting an
        // address alone still joins the list.
        body: new URLSearchParams({ name, email }),
        credentials: "omit",
      });
      setState(res.ok ? "done" : "error");
    } catch {
      setState("error");
    }
  };

  // WHAT ARRIVES AND WHAT IS AT THE END OF IT, which is the only thing still
  // unanswered once somebody is on the list, and where the client lives now.
  if (state === "done") {
    // BY NAME WHEN THERE IS ONE. It is the only thing the form now knows that
    // it did not before, and answering with it is what makes the field read as
    // having been asked rather than collected. First word only: a full name
    // read back at somebody is a receipt, not a greeting.
    //
    // AND ONLY WHEN IT IS THE SIZE OF A NAME. The field takes 128 characters
    // and people put addresses in the wrong box; a greeting is not the place to
    // find out. Anything longer falls back to the line that needs no name,
    // which also keeps this state inside the slot above.
    const word = name.trim().split(/\s+/)[0];
    const first = word.length <= 20 ? word : "";
    return (
      <>
        <p className="pb-confirm">
          {first ? `you're on the list, ${first}.` : "you're on the list."}
        </p>
        <p className="pb-status">
          you will receive an email when a spot opens. it walks you through
          setup, and the app is waiting at the end of it.
        </p>
      </>
    );
  }

  return (
    <>
      <form className="pb-rig" onSubmit={submit} {...handlers}>
        <input
          ref={nameRef}
          className="pb-rig-field pb-rig-name"
          type="text"
          name="name"
          required
          autoComplete="name"
          placeholder="your name"
          aria-label="your name"
          // The control plane's own ceiling is 128 CHARACTERS and this counts
          // UTF-16 units, so the two agree on every name and the browser is
          // the stricter of them on astral ones. Deliberately the stricter
          // side: a name stopped at the field beats one cut on the way in.
          maxLength={128}
          value={name}
          disabled={state === "busy"}
          onChange={(event) => setName(event.target.value)}
        />
        <div className="pb-rig-row">
          {chrome}
          <input
            className="pb-rig-field"
            type="email"
            name="email"
            required
            autoComplete="email"
            placeholder="you@example.com"
            aria-label="email address"
            value={email}
            disabled={state === "busy"}
            onChange={(event) => setEmail(event.target.value)}
          />
          <SubmitButton busy={state === "busy"} />
        </div>
      </form>
      {state === "error" && (
        <p className="pb-status pb-status-error">
          that didn't go through. give it a second and try again.
        </p>
      )}
    </>
  );
}


// MARK: - the "after": a fake sitrep

// Stand-ins for the SF Symbols the app draws: gauge, envelope, key, sliders,
// people and waveform in the side rail; eye, envelope.open, calendar,
// shippingbox, building.columns and receipt on the zones.
const GLYPHS = {
  sitrep: "M8 14.2A6.2 6.2 0 1 0 8 1.8a6.2 6.2 0 0 0 0 12.4Zm0-3.6 2.8-5M3.8 8h1M8 3.8v1m4.2 3.2h-1",
  mail: "M2 3.8h12v8.4H2V3.8Zm0 0 6 4.6 6-4.6",
  key: "M8 6.6a2.6 2.6 0 1 0 0-5.2 2.6 2.6 0 0 0 0 5.2Zm0 0v7.8l1.6-1.3M8 10.4h1.6",
  sliders: "M2 4.5h12M2 8h12M2 11.5h12M5 3.3v2.4M10.5 6.8v2.4M6.5 10.3v2.4",
  people: "M6 7.2a2.4 2.4 0 1 0 0-4.8 2.4 2.4 0 0 0 0 4.8Zm-4.2 6.4c.3-2.6 2-4 4.2-4s3.9 1.4 4.2 4M11 7.2a2 2 0 1 0 0-4m1.6 6.6c1 .5 1.6 1.7 1.8 3.8",
  pulse: "M1.5 8.5h3l1.6-4.4 3 8.4 1.8-4h3.6",
  gear: "M8 10.3a2.3 2.3 0 1 0 0-4.6 2.3 2.3 0 0 0 0 4.6ZM8 2v2m0 8v2M2 8h2m8 0h2M3.8 3.8l1.4 1.4m5.6 5.6 1.4 1.4m0-8.4-1.4 1.4m-5.6 5.6-1.4 1.4",
  eye: "M1.5 8S4 3.5 8 3.5 14.5 8 14.5 8 12 12.5 8 12.5 1.5 8 1.5 8Zm6.5 2a2 2 0 1 0 0-4 2 2 0 0 0 0 4Z",
  reading: "M2 6.5 8 2.5l6 4v7H2v-7Zm0 0 6 4 6-4",
  calendar: "M2.5 3.5h11v10h-11v-10Zm0 3h11M5 2v2.5M11 2v2.5",
  box: "M8 1.8 14 4.6v6.8L8 14.2 2 11.4V4.6L8 1.8ZM2 4.6 8 7.4l6-2.8M8 7.4v6.8",
  bank: "M2 6 8 2.5 14 6H2Zm1.2 0v6m3.2-6v6m3.2-6v6M12.8 6v6M2 13.5h12",
  receipt: "M3.5 1.8h9v12.4l-1.5-1-1.5 1-1.5-1-1.5 1-1.5-1-1.5 1V1.8Zm2.5 4h4m-4 3h4",
  retriage: "M13.5 8a5.5 5.5 0 1 1-1.6-3.9M12.5 1.8v2.6H9.9",
};

function Glyph({ name }: { name: keyof typeof GLYPHS }) {
  return (
    <svg viewBox="0 0 16 16" fill="none" stroke="currentColor" strokeWidth="1.25"
      strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
      <path d={GLYPHS[name]} />
    </svg>
  );
}

function ZoneHead({ glyph, title, count, sub }: {
  glyph: keyof typeof GLYPHS; title: string; count: number; sub?: string;
}) {
  return (
    <div className="pb-zone-h">
      <Glyph name={glyph} />
      {title} <em>{count}</em>
      {sub && <small>{sub}</small>}
    </div>
  );
}

// The app's avatar palette, dark halves (Palette.avatarPalette).
const AVATAR = {
  blue: ["#24384F", "#A9CBF0"], green: ["#22392E", "#9AD3B6"], rust: ["#442C26", "#EBA894"],
  violet: ["#2F2A4A", "#B8ACF2"], ochre: ["#3E2E1B", "#E2C078"], rose: ["#3E2839", "#E0A6CD"],
} as const;

// The mail from the "before" panel's scroll, as the sitrep has it: a name, the
// model's one-line abstraction of what the thread wants, and a chip only where
// there is a date. Overdue is the filled red chip plus the bar at the row's
// edge, exactly as SitrepView draws it; upcoming is an amber outline.
const EYES: Array<{
  who: string; initials: string; tone: keyof typeof AVATAR; line: string;
  chip?: string; overdue?: boolean;
}> = [
  { who: "Parkline Properties", initials: "PP", tone: "ochre", line: "Your lease renewal still needs a signature before the new rate lapses.", chip: "3d PAST DUE", overdue: true },
  { who: "Dr. Ortiz's Office", initials: "DO", tone: "green", line: "Confirm Thursday's 2:30 appointment or call to reschedule.", chip: "due today" },
  { who: "Jamie Chen", initials: "JC", tone: "blue", line: "Needs your pick between two offsite venues so they can hold the date.", chip: "due Fri" },
  { who: "Alex Rivera", initials: "AR", tone: "violet", line: "Replied to your offsite dates: the 14th works for everyone." },
  { who: "Mom", initials: "M", tone: "rose", line: "Sent photos from the weekend." },
];

// Newsletters from the scroll, one card per sender. Lettered tiles rather
// than anybody's real logo.
const READING: Array<[sender: string, count: number, blurb: string, tile: [bg: string, fg: string, mark: string]]> = [
  ["Medium Daily Digest", 7, "10 Habits of Highly Effective Engineers, and six more stories picked for you.", ["#f2f2ee", "#111", "M"]],
  ["Product Hunt Daily", 5, "An AI notetaker for your AI notetaker, and eight more launches.", ["#3a2a22", "#f08a5d", "P"]],
  ["Substack", 3, "Three new posts from writers you follow.", ["#f3eee6", "#c2531f", "S"]],
];

function AppMock() {
  return (
    <div
      className="pb-app"
      role="img"
      aria-label="The Passband sitrep. Two items for your eyes: a lease renewal three days past due and an appointment to confirm today, then three more conversations. Newsletters wait on a reading shelf, and a rail beside it holds the calendar, shipments, billing and receipts."
    >
      <div className="pb-app-bar">
        <div className="pb-lights" aria-hidden="true"><i /><i /><i /></div>
        <div className="pb-app-title">
          <b>passband</b>
          <small>SITREP</small>
        </div>
        <div className="pb-app-tools">
          <span className="pb-retriage"><Glyph name="retriage" />re-triage 7d</span>
          <span className="pb-need">2 need you now</span>
        </div>
      </div>

      <nav className="pb-side" aria-hidden="true">
        <span className="on"><Glyph name="sitrep" /></span>
        <span><Glyph name="mail" /></span>
        <span><Glyph name="key" /></span>
        <span><Glyph name="sliders" /></span>
        <span><Glyph name="people" /></span>
        <i className="pb-side-gap" />
        <span><Glyph name="pulse" /></span>
        <span><Glyph name="gear" /></span>
        <span className="pb-me">Y</span>
      </nav>

      <div className="pb-page">
        <div className="pb-dash-hero">
          <small>Good morning</small>
          <b>Two items for your eyes.</b>
        </div>

        <div className="pb-col">
          <section className="pb-zone">
            <ZoneHead glyph="eye" title="For your eyes" count={EYES.length} />
            {EYES.map(({ who, initials, tone, line, chip, overdue }, i) => {
              const [bg, fg] = AVATAR[tone];
              return (
                <div key={who} className={`pb-eye${overdue ? " overdue" : ""}${i === 0 ? " cursor" : ""}`}>
                  <span className="pb-avatar" style={{ background: bg, color: fg }}>{initials}</span>
                  <b>{who}</b>
                  <span className="line">{line}</span>
                  {chip && (
                    <span
                      className={`pb-chip${overdue ? " filled" : ""}`}
                      style={{ ["--c" as string]: overdue ? "var(--danger)" : "var(--warn)" }}
                    >
                      {chip}
                    </span>
                  )}
                </div>
              );
            })}
          </section>

          <section className="pb-zone">
            <ZoneHead glyph="reading" title="Reading" count={READING.length}
              sub="newsletters, announcements, and offers" />
            <div className="pb-reading">
              {READING.map(([sender, count, blurb, [bg, fg, mark]]) => (
                <div key={sender} className="pb-read">
                  <span className="pb-logo" style={{ background: bg, color: fg }}>{mark}</span>
                  <div className="pb-read-text">
                    <div className="pb-read-top">
                      <b>{sender}</b>
                      <span>{count} emails</span>
                    </div>
                    <p>{blurb}</p>
                  </div>
                </div>
              ))}
            </div>
          </section>
        </div>

        <aside className="pb-rail">
          <section className="pb-rec pb-rec-cal">
            <ZoneHead glyph="calendar" title="Calendar" count={2} />
            <div className="pb-rec-row"><b>Dr. Ortiz · checkup</b><span>Sep 26</span></div>
            <div className="pb-rec-row"><b>Team offsite</b><span>Oct 14</span></div>
          </section>
          <section className="pb-rec pb-rec-ship">
            <ZoneHead glyph="box" title="Shipments" count={2} />
            <div className="pb-ship">
              <div className="pb-ship-top">
                <i>UPS</i>
                <b>Keychron Q1</b>
                <span className="pb-chip filled" style={{ ["--c" as string]: "var(--warn)" }}>out for delivery</span>
              </div>
            </div>
            <div className="pb-ship">
              <div className="pb-ship-top">
                <i>a</i>
                <b>Linen notebooks</b>
                <span className="pb-chip filled" style={{ ["--c" as string]: "var(--accent)" }}>shipped</span>
              </div>
              <span className="pb-chip eta">arrives Sat</span>
            </div>
          </section>
          <section className="pb-rec pb-rec-bill">
            <ZoneHead glyph="bank" title="Billing" count={2} />
            <div className="pb-rec-row"><b>Chase ··4417</b><span className="pb-chip">statement</span></div>
            <div className="pb-rec-row"><b>Con Edison</b><span className="pb-chip">update</span></div>
          </section>
          <section className="pb-rec pb-rec-rcpt">
            <ZoneHead glyph="receipt" title="Receipts" count={3} />
            <div className="pb-rec-row"><b>Blue Bottle</b><span>$6.50</span></div>
            <div className="pb-rec-row"><b>DoorDash</b><span>$23.18</span></div>
            <div className="pb-rec-row"><b>Berghain</b><span>€25.00</span></div>
          </section>
        </aside>
      </div>
    </div>
  );
}

// MARK: - the page

// Where the analyzer draws, as fractions of the stage. Wide screens tune the
// band to the right of the copy; narrow ones centre it under the copy, low.
function scopeLayout(): ScopeLayout {
  const wide = innerWidth > 820;
  return wide
    ? { center: 0.7, base: 0.82, scale: 0.5, spread: 1 }
    : { center: 0.5, base: 0.9, scale: 0.28, spread: 1.9 };
}

// The carriers' names in the marker table, in the scope's order.
const MARKERS: Array<[who: string, status: string, tone: string]> = [
  ["Parkline Properties", "past due", "late"],
  ["Dr. Ortiz's Office", "today", "soon"],
  ["Jamie Chen", "due Fri", "soon"],
];

export function App() {
  const storyRef = useRef<HTMLElement>(null);
  const stageRef = useRef<HTMLDivElement>(null);
  const canvasRef = useRef<HTMLCanvasElement>(null);
  const [beat, setBeat] = useState(0);
  const [scrolled, setScrolled] = useState(false);
  const [locked, setLocked] = useState(false);
  const readoutRef = useRef<HTMLElement>(null);
  const [layout, setLayout] = useState<ScopeLayout>(() => scopeLayout());

  useEffect(() => {
    const canvas = canvasRef.current;
    const story = storyRef.current;
    if (!canvas || !story) return;
    const reduceMotion = matchMedia("(prefers-reduced-motion: reduce)").matches;
    const scene = createScope(canvas, {
      reduceMotion,
      layout: scopeLayout,
      // Written straight to the DOM: it changes every frame, and a re-render
      // per frame would buy nothing.
      onReadout: (squelch) => {
        if (readoutRef.current) readoutRef.current.textContent = `${Math.round(squelch * 100)}%`;
      },
    });

    // Progress through the story, 0 at the top to 1 where the stage releases.
    // The copy turns as the filter starts to close; the markers come up once
    // it has closed, when there is something left to point at.
    const onScroll = () => {
      const rect = story.getBoundingClientRect();
      const travel = Math.max(1, rect.height - innerHeight);
      const p = Math.min(1, Math.max(0, -rect.top / travel));
      scene?.setProgress(p);
      setBeat(p > 0.3 ? 1 : 0);
      setLocked(p > 0.58);
      setScrolled(p > 0.03);
    };
    const onResize = () => {
      setLayout(scopeLayout());
      onScroll();
    };
    onScroll();
    addEventListener("scroll", onScroll, { passive: true });
    addEventListener("resize", onResize);

    // Nothing to draw once the stage has scrolled away.
    const io = new IntersectionObserver(([entry]) => scene?.setVisible(entry.isIntersecting));
    io.observe(stageRef.current!);

    return () => {
      removeEventListener("scroll", onScroll);
      removeEventListener("resize", onResize);
      io.disconnect();
      scene?.destroy();
    };
  }, []);

  // THE WAITLIST IS A STATE OF THIS PAGE, not a document of its own: the URL
  // changes, so the link stays real and back still goes back, but nothing
  // remounts and the scope behind it never restarts.
  const [joining, setJoining] = useState(() => location.pathname === WAITLIST_PATH);
  useEffect(() => {
    const sync = () => setJoining(location.pathname === WAITLIST_PATH);
    addEventListener("popstate", sync);
    return () => removeEventListener("popstate", sync);
  }, []);
  const open = (next: boolean) => {
    if (next === joining) return;
    history.pushState(null, "", next ? WAITLIST_PATH : "/");
    setJoining(next);
  };
  const go = (next: boolean) => (event: MouseEvent<HTMLAnchorElement>) => {
    // Anything but a plain left click is asking for a new document: a new
    // tab, a new window, a saved link. Let the browser have those.
    if (event.metaKey || event.ctrlKey || event.shiftKey || event.altKey || event.button !== 0) return;
    event.preventDefault();
    open(next);
  };

  // The closing button goes back up to the hero and opens the rig there, so
  // there is only ever one form on the page.
  const backToTop = () => {
    const smooth = !matchMedia("(prefers-reduced-motion: reduce)").matches;
    scrollTo({ top: 0, behavior: smooth ? "smooth" : "auto" });
    open(true);
  };

  return (
    <main className="pb">
      <style href="pb-page" precedence="default">{PAGE_CSS_RESOLVED}</style>

      <section ref={storyRef} className="pb-story">
        <div
          ref={stageRef}
          className="pb-stage"
          data-locked={locked || undefined}
          data-scrolled={scrolled || undefined}
        >
          <canvas ref={canvasRef} className="pb-scene" aria-hidden="true" />
          <div className="pb-grat" />
          {CARRIERS.map((_, i) => {
            const { x, y } = markerPoint(i, layout);
            return (
              <span key={i} className="pb-mkr" style={{ left: `${x * 100}%`, top: `${y * 100}%` }}
                aria-hidden="true">{i + 1}</span>
            );
          })}
          <div className="pb-scrim" />
          <div className="pb-mkrs" aria-label="What made it through">
            <div><span>MKR</span><span>SENDER</span><span>STATUS</span></div>
            {MARKERS.map(([who, status, tone], i) => (
              <div key={who}>
                <b>{i + 1}</b>
                <span>{who}</span>
                <span className={tone}>{status}</span>
              </div>
            ))}
          </div>
          <span className="pb-read-l" aria-hidden="true">SPAN&nbsp;&nbsp;your inbox&nbsp;&nbsp;·&nbsp;&nbsp;4,312 msgs</span>
          <span className="pb-read-r" aria-hidden="true">SQUELCH&nbsp;<b ref={readoutRef}>0%</b></span>

          <header className="pb-top">
            <a className="pb-brand" href="/" onClick={go(false)}>
              <img src="/mark.svg" alt="" width={42} height={23} />
              <span>passband</span>
            </a>
            <nav className="pb-nav">
              <a href="/about">About</a>
              <a href="/self-host">Self-host</a>
              <a className="pb-keep" href="https://github.com/braelyn-ai/squelch">GitHub</a>
            </nav>
          </header>

          <div className="pb-copy">
            <div className="pb-beats" data-beat={beat}>
              <div className="pb-beat pb-beat-0" aria-hidden={beat !== 0}>
                <p className="pb-lede">Inbox zero every day was never realistic.</p>
                <h1 className="pb-hero">You’re only human.</h1>
                <p className="pb-sub">
                  Your attention is valuable. You deserve an inbox that treats it that way.
                </p>
              </div>
              <div className="pb-beat pb-beat-1" aria-hidden={beat !== 1}>
                <h2 className="pb-hero">Know what needs you.</h2>
                <p className="pb-sub">
                  Passband brings the important things forward, so you can give them your
                  attention and get on with your day.
                </p>
              </div>
            </div>
            <div className="pb-slot">
              {joining ? <Waitlist /> : <JoinButton onClick={go(true)} />}
              <p className="pb-fine">For Gmail, on Mac and iPhone.</p>
            </div>
          </div>

          <div className="pb-cue" aria-hidden="true">scroll</div>
        </div>
      </section>

      <section className="pb-section">
        <h2 className="pb-h2">
          Same inbox. <em>Squelched.</em>
        </h2>
        <p className="pb-intro">
          Every email still arrives. Passband reads each one, pulls forward the few that
          need you, and files the rest where you can find them when you want to.
        </p>
        <div className="pb-pair">
          <figure className="pb-panel pb-before">
            <figcaption>
              <b>Before</b> everything, in the order it arrived
              <span className="pb-unread" aria-label="4,312 unread">4,312</span>
            </figcaption>
            <div className="pb-window">
              <FakeInbox />
            </div>
          </figure>
          <div className="pb-gate" aria-hidden="true">
            <span>
              <svg viewBox="0 0 16 16" fill="none" stroke="currentColor" strokeWidth="1.6"
                strokeLinecap="round" strokeLinejoin="round">
                <path d="M2.5 8h11m0 0L9.5 4m4 4-4 4" />
              </svg>
            </span>
          </div>
          <figure className="pb-panel pb-after">
            <figcaption><b>After</b> what Passband shows you</figcaption>
            <div className="pb-window">
              <AppMock />
            </div>
          </figure>
        </div>
      </section>

      <section className="pb-section" style={{ paddingTop: 0 }}>
        <div className="pb-facts">
          <div className="pb-fact">
            <h3>Open source</h3>
            <p>
              MIT licensed. Every line is <a href="https://github.com/braelyn-ai/squelch">on GitHub</a>,
              the threat model included.
            </p>
          </div>
          <div className="pb-fact">
            <h3>Run it where you like</h3>
            <p>
              <a href="/self-host">Self-host</a> the daemon on your own machine, or let us run an
              isolated instance for you.
            </p>
          </div>
          <div className="pb-fact">
            <h3>Reading can’t send</h3>
            <p>
              Sync holds a read-only Google credential. The one that can write lives apart,
              and the sync path can’t load it.
            </p>
          </div>
        </div>
      </section>

      <section className="pb-section pb-close">
        <h2 className="pb-h2">Find a little breathing room.</h2>
        <button className="pb-cta" type="button" onClick={backToTop}>
          <span className="pb-cta-label">join the waitlist</span>
          <Arrow />
        </button>
      </section>

      <footer className="pb-foot">
        <span>Passband</span>
        <nav>
          <a href="/about">About</a>
          <a href="/self-host">Self-host</a>
          <a href="/privacy">Privacy</a>
          <a href="/terms">Terms</a>
          <a href="https://github.com/braelyn-ai/squelch">GitHub</a>
        </nav>
      </footer>
    </main>
  );
}
