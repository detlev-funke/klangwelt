// Context help: detailed texts on the controls. For an element,
// the entry whose selector matches the element itself or the nearest
// ancestor applies (the deepest wins). If an entry is missing, the
// help window at least shows the tooltip (title) and the window's entry.

import { t } from "./i18n.js";
import { keyLabel } from "./noteinput.js";

export interface HelpTopic {
  title: string;
  /** Paragraphs; simple emphasis with **bold**. */
  text: string[];
  /** Keys and mouse gestures: [key, effect]. */
  keys?: [string, string][];
}

type Entry = { sel: string; topic: HelpTopic | ((el: Element, x: number, y: number) => HelpTopic) };

const RULER_H = 32;

const ruler: HelpTopic = {
  title: t("Ruler"),
  text: [
    t("Bars and beats of the timeline. Dragging moves the view, a click sets the playback position."),
    t("With Shift or Ctrl you select a **time range**: Space then plays exactly that (looped if loop is on), the export can write just that range, and the agent is told about it with every message."),
    t("**Del** deletes the selected time range from all tracks: everything after it moves up, clips crossing the edges are split or shortened (recordings too) – a single undo step."),
    t("When harmony is shown, the lower band displays the chords in the colors of their mood – red edges are harmonic breaks."),
  ],
  keys: [
    [t("Drag"), t("Move the view")],
    [t("Click"), t("Set playback position")],
    [t("Shift/Ctrl + drag"), t("Select a time range (Alt: off grid)")],
    [t("Drag edge"), t("Adjust the time range")],
    [t("Shift + click"), t("Clear the time range")],
    [t("Del"), t("Delete the time range from all tracks (the rest moves up)")],
    [t("Right-click"), t("Menu: delete time range, clear selection")],
  ],
};

const lanes: HelpTopic = {
  title: t("Timeline"),
  text: [
    t("The clips of all tracks over time. Dragging clips moves them (they snap to beats), the right edge changes the length."),
    t("In an instrument track's **piano roll** you edit the notes directly: Alt+click adds a note, dragging moves the selection, the right edge changes the length. Right-click on selected notes: **quantize** (1/4, 1/8, 1/16 or the view grid – starts only, or start and length), transpose, delete."),
    t("A **double-click** on an empty lane of the piano roll also adds a note there – on the grid, as long as the note you set last. Next to a clip the clip grows by whole bars; a track without a note clip gets one."),
    t("With the **keyboard entry** switch in the track header you type notes: the letter keys play the visible octave like a piano and add the note at the violet mark."),
    t("Just drag audio files (WAV, AIFF, FLAC, Opus) here: each becomes an audio track with a clip starting at the spot under the mouse – dropped onto an audio track, the first file lands in it. Klangwelt can't read MP3 and AAC yet."),
    t("Recordings and inserted audio files have a **♪ → Notes** button: it creates the detected pitches as a melody track below. Right-click also offers a **rhythm track** (hits as kick, snare, hi-hat)."),
  ],
  keys: [
    [t("Ctrl + mouse wheel"), t("zoom")],
    [t("Shift + mouse wheel"), t("scroll horizontally")],
    [t("Shift while dragging"), t("off grid")],
    [t("Alt + click"), t("Add note (drag: length)")],
    [t("Double-click in the piano roll"), t("Add note")],
    [t("Double-click (empty space)"), t("new note clip")],
    [t("Double-click on audio"), t("Open analysis")],
    [t("Del"), t("Delete selection – after selecting a time range: remove it from all tracks")],
    ["↑ / ↓", t("move selected notes by a semitone (Shift: octave)")],
    [t("Right-click"), t("Menu: quantize, transpose, convert, delete")],
    [t("Drag a file in"), t("Insert an audio file as a track")],
  ],
};

const panelTopics: Record<string, HelpTopic> = {
  spuren: {
    title: t("Tracks"),
    text: [
      t("The main window: track headers on the left, the timeline on the right. Audio tracks contain recordings, instrument tracks contain notes played by a sound generator from the sound workshop."),
      t("In the header at the top you create new tracks and show the harmony."),
    ],
  },
  clip: {
    title: t("\"Clip\" window"),
    text: [
      t("Properties of the selected clip. For audio: source tempo and sync (it then plays at the project tempo), detect tempo, analyze, turn the melody into an instrument track."),
      t("For notes: the **performance markings** – dynamics pp … ff, crescendo, accent, staccato, legato, vibrato, brightness, scoop. They apply to the selected notes or, with nothing selected, to all of them."),
    ],
  },
  analyse: {
    title: t("\"Analysis\" window"),
    text: [
      t("What's inside a recording: pitch curve, detected notes, tempo and key. Opens with \"Analyze\" in the Clip window or by double-clicking an audio clip."),
      t("The detected melody can be turned into notes and searched for in the library."),
    ],
  },
  klang: {
    title: t("Sound workshop (\"Sound\")"),
    text: [
      t("The sound of the selected instrument track. At the top the instrument (pluck, organ, harpsichord, bass, synth, drums), below it three **fields**: character, timbre and space. A dot in a field or a named starting point sets only that field's controls."),
      t("**Fine-tuning** has every control individually, with the measurements on the right. Under **My sounds** you save settings for all projects."),
    ],
  },
  rhythmus: {
    title: t("\"Rhythm\" window"),
    text: [
      t("Drum patterns of typical genres and a step grid: one row per drum, 16 sixteenths per bar. Without a selected drum clip, the grid is a draft that \"Write to track\" creates; with a clip selected, every click edits that clip."),
    ],
  },
  song: {
    title: t("\"Song\" window"),
    text: [
      t("Complete pieces from templates (Trance, Ambient, Deep House, Synthwave, Lo-Fi). The mini editor shows the structure: columns are sections, rows are roles (pad, arpeggio, bass…), each cell is off, sparse or full."),
      t("**Generate song** writes the tracks; generating again replaces them – so you can quickly try out different structures."),
    ],
  },
  bibliothek: {
    title: t("\"Library\" window"),
    text: [
      t("Scores (MusicXML, Kern…) to search and listen to. A library is an index; the active one is searched. Get packs like Bach chorales or PDMX via \"Download\", your own files via \"Add\"."),
      t("Melody search finds pieces from notes or a sung recording. Below, the notes of the selected score with their own timeline."),
    ],
  },
  fuge: {
    title: t("\"Fugue\" window"),
    text: [
      t("Tools for fugue and counterpoint: answer (real or tonal), transformations (inversion, retrograde, augmentation…), strettos, countersubjects and a complete fugue plan for 2–4 voices. The subject is a note clip."),
      t("The **counterpoint checker** finds parallels, voice crossings and unprepared dissonances and marks them in the timeline."),
    ],
  },
  harmonie: {
    title: t("Harmony (circle of fifths)"),
    text: [
      t("Helps you write chord progressions. The circle shows major on the outside, minor in the middle, diminished on the inside; the best continuations carry rank numbers and arrows in the colors of their mood."),
      t("The progression follows the playback position. Clicking a chord plays the transition, double-clicking adds it; \"Write to track\" puts the new chords in – **Insert** makes room, **Replace** overwrites."),
    ],
  },
  agent: {
    title: t("\"Agent\" window"),
    text: [
      t("Tasks for a connected AI agent (e.g. Claude Code via MCP). Every message includes the current state: which tracks are audible, muted or soloed and which time range is selected – so \"make this part quieter\" works."),
      t("The top explains how to connect the agent and make it listen."),
      t("Each task shows its number and who took it (\"claimed by …\"). If the agent doesn't confirm a task within a minute, it's offered again – nothing gets lost. **Stop** at a task (or the message \"Stop\", \"Stop #3\") cancels it if nobody has picked it up yet, otherwise the agent is told with its next tool call."),
      t("In the browser version an agent connects through a browser tool: \"Claude in Chrome\", or Playwright MCP with the \"Playwright MCP Bridge\" extension (also in Vivaldi, Brave or Edge)."),
    ],
    keys: [[t("Enter"), t("send")], [t("Shift + Enter"), t("new line")]],
  },
  hilfe: {
    title: t("Help"),
    text: [t("Point at something with the mouse – this shows what it does. **F1** pins the text (lock icon); pressing F1 again on the same element releases it.")],
  },
};

const ENTRIES: Entry[] = [
  // ------------------------------------------------ Top bar
  { sel: "kw-transport .brand", topic: { title: "Klangwelt", text: [t("Make music, record, detect and design sounds – all in one program. The **intro tour** on the page of an empty project shows all the windows.")] } },
  { sel: "[data-help=datei]", topic: { title: t("\"File\" menu"), text: [t("New project, Open, Save as and Export. Projects are stored in Documents/Klangwelt/Projekte; \"Choose file…\" in the Open dialog loads a .klangwelt file from anywhere. A .klangwelt file contains everything, including the recordings.")], keys: [[t("Ctrl+N"), t("new")], [t("Ctrl+O"), t("open")], [t("Ctrl+Shift+S"), t("save as")]] } },
  { sel: ".save-btn", topic: { title: t("Save"), text: [t("Saves the project to its file; the first time, Klangwelt asks for a name. A dot on the button and \"•\" after the name indicate unsaved changes.")], keys: [[t("Ctrl+S"), t("save")]] } },
  { sel: ".mi-export", topic: { title: t("Export"), text: [t("Writes the piece to a file – exactly as it sounds (muted tracks are left out, solo applies):"), t("**WAV** uncompressed · **FLAC** losslessly compressed · **Opus** small for your phone (selectable bitrate, default 160 kbit/s) · **Stems** one WAV per track · **MIDI** notes only."), t("If a time range is selected, the export can write just that. Location: Music/Klangwelt/Exporte.")], keys: [[t("Ctrl+E"), t("Export dialog")]] } },
  { sel: ".proj-name", topic: { title: t("Project name"), text: [t("Name of the project file; \"•\" means unsaved changes. Hover to see the full path in the tooltip.")] } },
  { sel: "kw-transport .btn.big.rec", topic: { title: t("Record"), text: [t("Records from the microphone and creates the recording as an audio clip. Afterwards you can analyze it, detect its tempo or turn the melody into notes.")], keys: [["R", t("Start / stop recording")]] } },
  { sel: "kw-transport .btn.big.primary", topic: { title: t("Playback"), text: [t("Plays or pauses. If a time range is selected in the timeline, it plays exactly that – looped if loop is on. Without a selection, it stops right at the end of the piece and the playhead stays there, so keyboard entry goes on at that spot; Play from the end starts again at the beginning.")], keys: [[t("Space"), t("Play / pause")]] } },
  { sel: "kw-transport .btn.big", topic: { title: t("Stop"), text: [t("Stops and resets the playback position and the view to the start.")], keys: [[t("Enter / Home"), t("Stop")]] } },
  { sel: "kw-transport .lcd", topic: { title: t("Position"), text: [t("Top: bar.beat.sixteenth, bottom: the time in minutes and seconds.")] } },
  { sel: "[data-help=tempo]", topic: { title: t("Tempo"), text: [t("Project tempo in beats per minute (BPM). Instrument tracks follow immediately; audio clips with \"Sync\" are stretched without changing their pitch.")] } },
  { sel: "[data-help=takt]", topic: { title: t("Time signature"), text: [t("The project's time signature (e.g. 4/4). It determines bar lines, grid and metronome.")] } },
  { sel: "[data-help=metronom]", topic: { title: t("Metronome"), text: [t("A click on every beat, accented on the one. Klangwelt remembers the setting until the next start.")] } },
  { sel: "[data-help=wiederholen]", topic: { title: t("Loop"), text: [t("Plays in a loop: the selected time range or, without a selection, the whole piece (rounded up to whole bars).")], keys: [["L", t("Loop on/off")]] } },
  { sel: "[data-help=rueckgaengig]", topic: { title: t("Undo"), text: [t("Takes back the last step – including changes made by the agent. The tooltip names the step.")], keys: [[t("Ctrl+Z"), t("undo")]] } },
  { sel: "[data-help=wiederherstellen]", topic: { title: t("Redo"), text: [t("Brings back an undone step.")], keys: [[t("Ctrl+Y / Ctrl+Shift+Z"), t("redo")]] } },
  { sel: "[data-help=fenster]", topic: { title: t("Windows"), text: [t("Shows and hides windows and resets the layout. Drag a window by its tab to move it elsewhere: dock it, group it as a tab or let it float freely.")] } },
  { sel: "[data-help=einstellungen]", topic: { title: t("Settings"), text: [t("Basic settings, e.g. the pitch axis: low notes at the top (like a piano turned sideways) or high notes at the top (like in sheet music).")] } },
  { sel: "canvas.meter", topic: { title: t("Level"), text: [t("Output on top, microphone below. Red means too loud – lower the track levels.")] } },
  { sel: ".chip.agent", topic: { title: t("Agent"), text: [
    t("Shows whether an AI agent (e.g. Claude Code) is connected to Klangwelt:"),
    t("**Agent listening** – it picks up tasks from the chat right away. **Agent working on #n** – a task is in progress, or the agent called a tool within the last minute. **Agent ready** – an agent is connected but quiet. **No agent connected** – nothing for more than a minute; the app's endpoint is waiting. **Agent off** – the endpoint couldn't start."),
    t("A click opens the agent chat with the prompt for connecting: copy it, paste it into Claude Code, done."),
  ] } },
  { sel: "[data-help=ausgabegeraet]", topic: { title: t("Output device"), text: [
    t("The device Klangwelt plays through (speakers, headphones, audio interface) and its sample rate."),
    t("A click opens the list: **System default** follows the default device of Windows or macOS, below it are all devices. The choice takes effect immediately – playback in progress continues on the new device – and also applies the next time you start. If the chosen device isn't connected then, Klangwelt plays through the system default and briefly lets you know."),
    t("Without a device, Klangwelt keeps running without sound."),
  ] } },
  { sel: "[data-help=aufnahmegeraet]", topic: { title: t("Recording device"), text: [
    t("The microphone for recordings and for singing melodies into the library search. A click opens the list: **System default** or a specific device (e.g. a USB microphone or audio interface)."),
    t("The choice applies from the next recording on and also after a restart; it can't be changed during a recording."),
  ] } },

  // ------------------------------------------------ Tracks
  { sel: ".harmony-toggle", topic: { title: t("Harmony in the timeline"), text: [t("Shows a band of chords in the ruler, colored by their mood (resolving, tense, surprising …). Red edges mark harmonic breaks – out-of-key chords without an explanation, or tritone leaps. The band also appears while you're working in the Harmony window.")] } },
  { sel: "[data-help=neue-audiospur]", topic: { title: t("Audio track"), text: [t("New track for recordings. Start recording with R or the red button at the top.")] } },
  { sel: "[data-help=neue-instrumentspur]", topic: { title: t("Instrument track"), text: [t("New track with an instrument and a short pattern: pluck, organ, harpsichord, bass, synth or drums. Shape the sound in the \"Sound\" window.")] } },
  { sel: ".tracks-head", topic: panelTopics.spuren },
  { sel: ".track-row input.text", topic: { title: t("Track name"), text: [t("Click and type to rename the track; Enter confirms. The agent refers to tracks by name and ID.")] } },
  { sel: ".track-row .toggle.m", topic: { title: t("Mute (M)"), text: [t("Mutes the track. Muted tracks are also left out of the export; the agent knows which ones are muted.")] } },
  { sel: ".track-row .toggle.s", topic: { title: t("Solo (S)"), text: [t("Only solo tracks are audible – several at once, too. Also applies to the export.")] } },
  { sel: ".track-row .toggle.convert", topic: { title: t("Convert to notes"), text: [t("Appears on audio tracks in pitch view: creates the detected notes of all the track's clips as a melody track directly below – quantized to sixteenths, low registers as a bass track. One undo step."), t("For drum recordings: right-click the clip → \"Convert to rhythm track\" (kick, snare, hi-hat).")] } },
  { sel: ".track-row .toggle.v.x", topic: { title: t("Expression lane"), text: [t("Shows a lane for expression below the notes: velocity, vibrato, brightness or bend per note. Drag values or paint across several notes; Shift draws a straight line (crescendo), double-click resets.")] } },
  { sel: ".track-row .toggle.v", topic: { title: t("View"), text: [t("Switches between a compact row and the piano roll. In the piano roll, audio tracks show what the analysis detects.")] } },
  { sel: ".track-row .stepper", topic: { title: t("Pitch"), text: [t("Shifts the track in semitones (−24 … +24). For audio the tempo stays the same; the shift is included in the export and in MIDI.")] } },
  { sel: "[data-help=pegel]", topic: { title: t("Level"), text: [t("Track volume in dB (−60 … +6). Double-click sets 0 dB.")] } },
  { sel: "[data-help=panorama]", topic: { title: t("Pan"), text: [t("Position in the stereo field, from left to right. Double-click centers it.")] } },
  { sel: ".track-row canvas.keys", topic: { title: t("Keyboard"), text: [t("A piano rotated by 90°: click plays a note. Change the pitch axis (low or high notes at the top) in the gear menu."), t("With keyboard entry on, a click adds the note at the violet mark – like the letter keys shown on the piano.")], keys: [[t("Mouse wheel"), t("scroll pitch range")], [t("Ctrl + mouse wheel"), t("zoom")], [t("Double-click"), t("fit")]] } },
  {
    sel: ".track-row .entry-switch",
    // a function: the letters of the octave keys come from the keyboard layout (arrives later)
    topic: () => ({
      title: t("Keyboard entry"),
      text: [
        t("Type notes instead of drawing them. While the switch is on, the letter keys play the octave visible in the piano roll – the middle letter row are the white keys, the row above the black ones; the piano shows each key's letter. Every note lands at the **violet mark** in the timeline, which then moves on by one grid step (the grid follows the zoom)."),
        t("Hold **Shift** to stay in place: that's how you type chords. A click on a key of the piano works like its letter. Each note is its own undo step."),
        t("The mark starts at the playhead or at the selected time range; a click into the piano roll or the ruler moves it."),
        t("While the mode is on, the letter keys don't trigger their usual shortcuts (L loop, R record); Space and the Ctrl shortcuts (undo, save …) still work."),
      ],
      keys: [
        [t("Letter keys"), t("Add a note and move on")],
        [t("Shift + letter"), t("Add a note, stay in place (chord)")],
        [t("Click on the piano"), t("Add this note")],
        [`${keyLabel("KeyZ")} / ${keyLabel("KeyX")}`, t("Octave down / up")],
        ["← / →", t("Move the mark one step (Shift: one bar)")],
        [t("Backspace"), t("One step back, delete the notes there")],
        [t("Esc"), t("End keyboard entry")],
      ],
    }),
  },
  { sel: ".track-row .expr-select", topic: { title: t("Expression type"), text: [t("What the expression lane shows: velocity (loudness), vibrato, brightness or bend (scoop).")] } },
  { sel: ".track-row", topic: { title: t("Track header"), text: [t("A track's name, mute/solo, view, level, pan and pitch. Drag the line between two tracks to change the height (double-click: default).")] } },
  { sel: "kw-timeline canvas", topic: (el, _x, y) => (y - el.getBoundingClientRect().top < RULER_H ? ruler : lanes) },
  { sel: ".empty-hint .song-start", topic: { title: t("Start with a song"), text: [t("Creates a complete piece from a template (Trance, Ambient, Deep House, Synthwave, Lo-Fi). Keep shaping its structure in the \"Song\" window.")] } },
  { sel: ".empty-hint .beat-start", topic: { title: t("Start with a beat"), text: [t("Creates a drum track with four bars of a genre pattern and adopts its tempo and sound. Keep editing it in the \"Rhythm\" window.")] } },
  { sel: ".empty-hint .intro-btn", topic: { title: t("Intro tour"), text: [t("A tour of all windows: highlighted step by step, with notes on the most important features. Quit anytime with Esc.")] } },
  { sel: ".empty-hint", topic: { title: t("Empty project"), text: [t("Load the demo, start with a song or beat, add a track or record right away. The intro tour shows you all the windows.")] } },
  { sel: "kw-timeline", topic: lanes },

  // ------------------------------------------------ Clip
  { sel: "kw-panel[tab=clip] .mark.dyn", topic: { title: t("Dynamics"), text: [t("Sets the velocity of the selected notes (no selection: all notes in the clip): pp 32, p 48, mp 64, mf 80, f 100, ff 120.")] } },
  { sel: "kw-panel[tab=clip] .mark", topic: { title: t("Expression marks"), text: [t("Crescendo/Decrescendo, Accent (+25), Staccato (half length), Legato (up to the next note), Vibrato, Brighter/Darker, Scoop and Neutral – for the selected notes or all of them.")] } },
  { sel: "kw-panel[tab=clip] input[type=range]", topic: { title: t("Expression sliders"), text: [t("Show the average of the selected notes; dragging sets all of them to that value (one undo step).")] } },

  // ------------------------------------------------ Sound
  { sel: "kw-panel[tab=klang] .snd-kinds", topic: { title: t("Instrument"), text: [t("Changes the track's instrument. The notes stay; the sound starts from the instrument's starting point.")] } },
  { sel: "kw-panel[tab=klang] .snd-mark", topic: { title: t("Starting point"), text: [t("Named point in the field – a click sets this field's controls; the other fields stay untouched.")] } },
  { sel: "kw-panel[tab=klang] .snd-field[data-field=charakter]", topic: { title: t("\"Character\" field"), text: [t("Each instrument has its own axes (for drums, e.g. crisp → powerful and simple → complex). A point here sets many of the instrument's controls at once.")] } },
  { sel: "kw-panel[tab=klang] .snd-field[data-field=farbe]", topic: { title: t("\"Timbre\" field"), text: [t("Horizontal: dark → bright (tilt filter); vertical: smooth → rough (flutter, saturation, grain).")] } },
  { sel: "kw-panel[tab=klang] .snd-field[data-field=raum]", topic: { title: t("\"Space\" field"), text: [t("Horizontal: narrow → wide (stereo, the lows stay centered); vertical: near → far (reverb; quieter and duller from a distance).")] } },
  { sel: "kw-panel[tab=klang] .snd-field", topic: { title: t("Sound field"), text: [t("Drag the point or click a starting point. Each field only sets its own controls.")] } },
  { sel: "kw-panel[tab=klang] .snd-play", topic: { title: t("Preview"), text: [t("What plays briefly after you release a control: nothing, a single note or the pattern. Your choice is remembered.")] } },
  { sel: "kw-panel[tab=klang] .snd-wave", topic: { title: t("Over time"), text: [t("Waveform of a test tone. For the synth, you draw the envelope here: volume and brightness as curves over time – drag points; double-click adds or deletes a point.")] } },
  { sel: "kw-panel[tab=klang] .snd-fine", topic: { title: t("Fine-tuning"), text: [t("All of the instrument's controls individually. On the right, the sound's measurements (brightness, decay, fundamental …) – the same ones the agent reads.")] } },
  { sel: "kw-panel[tab=klang] .snd-lib", topic: { title: t("My sounds"), text: [t("Saves the current settings under a name – all controls, the position in the fields and, for the synth, the envelope. Saved sounds are available in every project; a click loads one into the track. ✎ renames, the trash can deletes on the second click.")] } },

  // ------------------------------------------------ Rhythm
  { sel: ".rh-genre", topic: { title: t("Genre"), text: [t("Typical patterns of a style with tempo, recommended swing and a tip. A click shows the genre's patterns.")] } },
  { sel: ".rh-pattern", topic: { title: t("Pattern"), text: [t("Loads the pattern into the grid – as a draft or, if a drum clip is selected, as the clip's new content.")] } },
  { sel: ".rh-step", topic: { title: t("Step"), text: [t("A click sets or clears a hit. The bright columns are beats 1 2 3 4. Klangwelt adds the accents itself (loud backbeat, quiet ghost notes).")] } },
  { sel: ".rh-label", topic: { title: t("Drum"), text: [t("A click plays the drum.")] } },
  { sel: ".rh-swing", topic: { title: t("Swing"), text: [t("Delays every second sixteenth – from straight to heavily swung.")] } },
  { sel: ".rh-write", topic: { title: t("Write to track"), text: [t("Creates the draft as a clip: after the last clip of a drum track, or in a new track with the genre's sound.")] } },

  // ------------------------------------------------ Song
  { sel: ".song-tpl", topic: { title: t("Template"), text: [t("Loads a style's structure, tempo, key and chords into the editor. Tracks are only written by \"Generate song\".")] } },
  { sel: ".song-cell", topic: { title: t("Cell"), text: [t("How strongly a role plays in a section: off → sparse → full. Sparse means e.g. an arpeggio in eighths instead of sixteenths.")], keys: [[t("Click"), t("more")], [t("Shift + click"), t("less")]] } },
  { sel: ".song-chords", topic: { title: t("Chords"), text: [t("Chords in sequence – lower case is minor (a = A minor, C = C major; F♯ or F#, B♭ or Bb) – or scale degrees (i, IV, V …). Next to them, the bars per chord.")] } },
  { sel: ".song-write", topic: { title: t("Generate song"), text: [t("Writes one track per role (all drums together in \"Beat\") and one clip per section, sets the tempo and zooms to the whole song. Generating again replaces the most recently generated tracks – one undo step brings them back.")] } },

  // ------------------------------------------------ Library
  { sel: ".lib-name", topic: { title: t("Libraries"), text: [t("Name of the active library. A click opens the manager: switch, create a new one (e.g. for experimenting), rename, delete (only the index, never scores).")] } },
  { sel: "[data-help=lib-herunterladen]", topic: { title: t("Download"), text: [t("Opens the catalog of music libraries: PDMX (over 250,000 scores), OpenScore, KernScores with Bach, Mozart and Beethoven, and DCML with harmonic analysis. Each package lists its size and license; a click downloads it and adds it to the active library – in the background, even across a restart."), t("If a download is already running, the button says \"Queue\": the package goes into the queue and starts by itself afterwards – so you can order several packages at once.")] } },
  { sel: "[data-help=lib-warteschlange]", topic: { title: t("Queue"), text: [t("Ordered packages that are downloaded and added after the current job – one after another, each into the library that was active when it was ordered."), t("The × removes a package; \"Remove all\" clears the list. \"Cancel\" only stops the current job; then the next one starts. The queue is lost when you close the app.")] } },
  { sel: "canvas.lib-ruler", topic: { title: t("Score timeline"), text: [t("Drag to move through time, a click sets the cursor, Shift/Ctrl+drag marks a section to listen to or import.")] } },
  { sel: "canvas.lib-roll", topic: { title: t("Score notes"), text: [t("All voices of the selected score.")], keys: [[t("Mouse wheel"), t("scroll")], [t("Ctrl + mouse wheel"), t("zoom")], [t("middle mouse button"), t("drag")]] } },

  // ------------------------------------------------ Harmony
  { sel: "kw-panel[tab=harmonie] .hc-mode", topic: { title: t("Insert or replace"), text: [t("**Insert** makes room: everything from that point on moves later in all tracks by the length of the new chords. **Replace** overwrites what follows in the target track.")] } },
  { sel: "kw-panel[tab=harmonie] .hc-follow", topic: { title: t("Follow"), text: [t("During playback and when you set the cursor, the progression picks up the chords at that point – your own new chords stay after them.")] } },
  { sel: "kw-panel[tab=harmonie] .hc-chip", topic: { title: t("Progression"), text: [t("The chords so far. A click plays one; \"Write to track\" writes the latest new chords into the target track.")] } },
  { sel: "kw-panel[tab=harmonie] .hc-pick", topic: { title: t("Continuation"), text: [t("A suggestion with scale degree, function and reasoning. Click plays the transition, double-click adds it to the progression. The dot in front shows the mood.")] } },
  { sel: "kw-panel[tab=harmonie] .hc-legend", topic: { title: t("Moods"), text: [t("resolving (green), tense (orange), surprising (pink), bright and dark – hover over a mood to highlight only its transitions.")] } },
  { sel: "kw-panel[tab=harmonie] .hc-circle", topic: { title: t("Circle of fifths"), text: [t("Major outside, minor in the middle, diminished inside; the key's chords are colored. The best continuations show a rank and an arrow.")], keys: [[t("Click"), t("preview transition")], [t("Double-click"), t("add to progression")]] } },

  // ------------------------------------------------ Context menu
  { sel: ".ctx-menu [data-act^=raster]", topic: { title: t("Quantize"), text: [t("Moves the selected notes onto the grid: 1/4, 1/8, 1/16 or the view's grid (finer the further you zoom in). The grid follows the song's bars, even if the clip starts offset."), t("\"Quantize\" only moves the starts, \"Start and length\" also the ends – every note stays at least one grid step long.")] } },
  { sel: ".ctx-menu", topic: { title: t("Context menu"), text: [t("What you can do with the clicked item: quantize and transpose notes, convert a recording into a melody or rhythm track, delete the selected time range from all tracks. Esc or a click outside closes it.")] } },

  // ------------------------------------------------ Agent
  { sel: ".chat-input", topic: { title: t("Task"), text: [t("Write what the agent should do – e.g. \"Make the kick deeper\" or \"Write a bassline for the selected bars\". Audible tracks and the selected time range are sent along automatically.")], keys: [[t("Enter"), t("send")], [t("Shift + Enter"), t("new line")]] } },

  // ------------------------------------------------ Windows in general
  { sel: ".dk-tab", topic: { title: t("Window tab"), text: [t("Dragging moves the window: onto another window's target cross to dock, onto a tab bar to group, anywhere else to float. The × or the middle mouse button closes it; \"Windows ▾\" brings it back.")], keys: [[t("Esc while dragging"), t("cancel")]] } },
  { sel: "kw-panel[tab=clip]", topic: panelTopics.clip },
  { sel: "kw-panel[tab=analyse]", topic: panelTopics.analyse },
  { sel: "kw-panel[tab=klang]", topic: panelTopics.klang },
  { sel: "kw-panel[tab=rhythmus]", topic: panelTopics.rhythmus },
  { sel: "kw-panel[tab=song]", topic: panelTopics.song },
  { sel: "kw-panel[tab=bibliothek]", topic: panelTopics.bibliothek },
  { sel: "kw-panel[tab=fuge]", topic: panelTopics.fuge },
  { sel: "kw-panel[tab=harmonie]", topic: panelTopics.harmonie },
  { sel: "kw-panel[tab=agent]", topic: panelTopics.agent },
  { sel: "main.workspace", topic: panelTopics.spuren },
  { sel: "kw-help", topic: panelTopics.hilfe },
];

export interface HelpView {
  topic: HelpTopic;
  /** Tooltip of a deeper element without an entry of its own ("Here: …"). */
  tip: string | null;
  /** Element the help applies to (for recognizing it when freezing). */
  el: Element;
}

function tooltipOf(el: Element): string {
  return (el.getAttribute("title") || el.getAttribute("aria-label") || "").trim();
}

/** Help for an element (with mouse position for areas like the timeline). */
export function helpFor(target: Element | null, x = 0, y = 0): HelpView | null {
  let tip: string | null = null;
  for (let el: Element | null = target; el; el = el.parentElement) {
    const entry = ENTRIES.find((e) => el!.matches(e.sel));
    if (entry) {
      const topic = typeof entry.topic === "function" ? entry.topic(el, x, y) : entry.topic;
      const own = tooltipOf(el);
      // tooltip of a deeper element or dynamic tooltip of the element itself
      const extra = tip ?? (own && !topic.text.some((p) => p.includes(own)) ? own : null);
      return { topic, tip: extra && extra !== topic.title ? extra : null, el };
    }
    if (!tip) {
      const own = tooltipOf(el);
      if (own) tip = own;
    }
  }
  return null;
}

export const HELP_COUNT = ENTRIES.length;
