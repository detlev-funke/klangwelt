// Kontexthilfe: ausführliche Texte zu den Bedienelementen. Zu einem Element
// gilt der Eintrag, dessen Selektor auf das Element selbst oder den nächsten
// Vorfahren passt (der tiefste gewinnt). Fehlt ein Eintrag, zeigt das
// Hilfefenster wenigstens den Tooltip (title) und den Eintrag des Fensters.

export interface HelpTopic {
  title: string;
  /** Absätze; einfache Hervorhebung mit **fett**. */
  text: string[];
  /** Tasten und Mausgesten: [Taste, Wirkung]. */
  keys?: [string, string][];
}

type Entry = { sel: string; topic: HelpTopic | ((el: Element, x: number, y: number) => HelpTopic) };

const RULER_H = 32;

const ruler: HelpTopic = {
  title: "Lineal",
  text: [
    "Takte und Schläge der Zeitleiste. Ziehen verschiebt die Ansicht, ein Klick setzt die Abspielposition.",
    "Mit Umschalt oder Strg markierst du einen **Zeitraum**: Die Leertaste spielt dann genau ihn (mit Wiederholen in Schleife), der Export kann nur ihn schreiben, und der Agent erfährt ihn mit jeder Nachricht.",
    "**Entf** löscht den gerade markierten Zeitraum aus allen Spuren: Alles danach rückt auf, Clips über die Kanten werden geteilt bzw. gekürzt (auch Aufnahmen) – ein Rückgängig-Schritt.",
    "Ist die Harmonie eingeblendet, zeigt das untere Band die Akkorde in den Farben ihrer Stimmung – rote Kanten sind harmonische Brüche.",
  ],
  keys: [
    ["Ziehen", "Ansicht verschieben"],
    ["Klick", "Abspielposition setzen"],
    ["Umschalt/Strg + Ziehen", "Zeitraum markieren (Alt: ohne Raster)"],
    ["Kante ziehen", "Zeitraum anpassen"],
    ["Umschalt + Klick", "Zeitraum aufheben"],
    ["Entf", "Zeitraum aus allen Spuren löschen (Rest rückt auf)"],
    ["Rechtsklick", "Menü: Zeitraum löschen, Markierung aufheben"],
  ],
};

const lanes: HelpTopic = {
  title: "Zeitleiste",
  text: [
    "Die Clips aller Spuren über der Zeit. Clips ziehen verschiebt sie (rastet auf Schläge ein), an der rechten Kante ändert sich die Länge.",
    "In der **Notenansicht** einer Instrumentspur bearbeitest du die Noten direkt: Alt+Klick setzt eine Note, Ziehen verschiebt die Auswahl, die rechte Kante ändert die Länge. Rechtsklick auf markierte Noten: **am Takt ausrichten** (1/4, 1/8, 1/16 oder Raster der Ansicht – nur die Anfänge oder Anfang und Länge), transponieren, löschen.",
    "Audiodateien (WAV, AIFF, FLAC, Opus) ziehst du einfach hierher: Jede wird eine Audiospur mit Clip ab der Stelle unter der Maus – auf eine Audiospur gezogen landet die erste Datei in ihr. MP3 und AAC liest Klangwelt noch nicht.",
    "Aufnahmen und eingefügte Audiodateien tragen den Knopf **♪ → Noten**: Er legt die erkannten Töne als Melodie-Spur darunter an. Per Rechtsklick geht auch eine **Rhythmus-Spur** (Anschläge als Bassdrum, Snare, Hi-Hat).",
  ],
  keys: [
    ["Strg + Mausrad", "zoomen"],
    ["Umschalt + Mausrad", "waagrecht scrollen"],
    ["Umschalt beim Ziehen", "ohne Raster"],
    ["Alt + Klick", "Note setzen (ziehen: Länge)"],
    ["Doppelklick (frei)", "neuer Noten-Clip"],
    ["Doppelklick auf Audio", "Analyse öffnen"],
    ["Entf", "Auswahl löschen – nach dem Markieren eines Zeitraums: ihn aus allen Spuren"],
    ["↑ / ↓", "markierte Noten um einen Halbton (Umschalt: Oktave)"],
    ["Rechtsklick", "Menü: ausrichten, transponieren, umwandeln, löschen"],
    ["Datei hineinziehen", "Audiodatei als Spur einfügen"],
  ],
};

const panelTopics: Record<string, HelpTopic> = {
  spuren: {
    title: "Spuren",
    text: [
      "Das Hauptfenster: links die Spurköpfe, rechts die Zeitleiste. Audiospuren enthalten Aufnahmen, Instrumentspuren Noten, die ein Klangerzeuger der Soundwerkstatt spielt.",
      "Oben im Kopf legst du neue Spuren an und blendest die Harmonie ein.",
    ],
  },
  clip: {
    title: "Fenster „Clip“",
    text: [
      "Eigenschaften des gewählten Clips. Bei Audio: Quelltempo und Synchronisieren (läuft dann im Projekttempo), Tempo erkennen, Analysieren, Melodie in eine Instrumentspur übernehmen.",
      "Bei Noten: die **Vortragszeichen** – Dynamik pp … ff, Crescendo, Akzent, Staccato, Legato, Vibrato, Helligkeit, Anschleifen. Sie gelten für die gewählten Noten oder, ohne Auswahl, für alle.",
    ],
  },
  analyse: {
    title: "Fenster „Analyse“",
    text: [
      "Was in einer Aufnahme steckt: Tonhöhenkurve, erkannte Noten, Tempo und Tonart. Öffnet sich mit „Analysieren“ im Clip-Fenster oder per Doppelklick auf einen Audio-Clip.",
      "Die erkannte Melodie lässt sich als Noten übernehmen und in der Bibliothek suchen.",
    ],
  },
  klang: {
    title: "Soundwerkstatt („Klang“)",
    text: [
      "Der Klang der gewählten Instrumentspur. Oben das Instrument (Zupfer, Orgel, Cembalo, Bass, Synth, Schlagzeug), darunter drei **Felder**: Charakter, Klangfarbe und Raum. Ein Punkt im Feld oder ein benannter Startpunkt setzt nur die Regler dieses Felds.",
      "Im **Feinschliff** stehen alle Regler einzeln, rechts die Messwerte. Unter **Meine Klänge** sicherst du Einstellungen für alle Projekte.",
    ],
  },
  rhythmus: {
    title: "Fenster „Rhythmus“",
    text: [
      "Schlagzeug-Muster typischer Genres und ein Schritt-Raster: eine Zeile je Trommel, 16 Sechzehntel je Takt. Ohne gewählten Schlagzeug-Clip ist das Raster ein Entwurf, den „In Spur schreiben“ anlegt; mit gewähltem Clip bearbeitet jeder Klick den Clip.",
    ],
  },
  song: {
    title: "Fenster „Song“",
    text: [
      "Ganze Stücke aus Vorlagen (Trance, Ambient, Deep House, Synthwave, Lo-Fi). Der Mini-Editor zeigt den Aufbau: Spalten sind Abschnitte, Zeilen sind Rollen (Fläche, Arpeggio, Bass …), jede Zelle ist aus, sparsam oder voll.",
      "**Song erzeugen** schreibt die Spuren; erneut erzeugen ersetzt sie – so probierst du den Aufbau schnell durch.",
    ],
  },
  bibliothek: {
    title: "Fenster „Bibliothek“",
    text: [
      "Partituren (MusicXML, Kern …) zum Suchen und Anhören. Eine Bibliothek ist ein Index; gesucht wird in der aktiven. Pakete wie Bach-Choräle oder PDMX lädst du über „Herunterladen“, eigene Dateien über „Hinzufügen“.",
      "Die Melodiesuche findet Stücke zu Noten oder einer gesungenen Aufnahme. Unten die Noten der gewählten Partitur mit eigener Zeitleiste.",
    ],
  },
  fuge: {
    title: "Fenster „Fuge“",
    text: [
      "Werkzeuge für Fuge und Kontrapunkt: Antwort (real oder tonal), Verwandlungen (Umkehrung, Krebs, Vergrößerung …), Engführungen, Gegensätze und ein ganzer Fugenplan für 2–4 Stimmen. Das Thema ist ein Noten-Clip.",
      "Der **Kontrapunkt-Prüfer** findet Parallelen, Stimmkreuzungen und unvorbereitete Dissonanzen und markiert sie in der Zeitleiste.",
    ],
  },
  harmonie: {
    title: "Harmonie (Quintenzirkel)",
    text: [
      "Hilft beim Schreiben von Akkordfolgen. Der Zirkel zeigt Dur außen, Moll in der Mitte, vermindert innen; die besten Fortsetzungen tragen Rangnummern und Pfeile in den Farben ihrer Stimmung.",
      "Der Verlauf folgt der Abspielposition. Klick auf einen Akkord spielt den Übergang vor, Doppelklick übernimmt ihn; „In Spur schreiben“ setzt die neuen Akkorde ein – **Einfügen** schafft Platz, **Ersetzen** überschreibt.",
    ],
  },
  agent: {
    title: "Fenster „Agent“",
    text: [
      "Aufträge an einen verbundenen KI-Agenten (z. B. Claude Code über MCP). Jede Nachricht trägt die Lage mit: welche Spuren hörbar, stumm oder solo sind und welcher Zeitraum markiert ist – „mach das hier leiser“ funktioniert also.",
      "Oben steht, wie du den Agenten verbindest und zuhören lässt.",
    ],
    keys: [["Enter", "senden"], ["Umschalt + Enter", "neue Zeile"]],
  },
  hilfe: {
    title: "Hilfe",
    text: ["Zeig mit der Maus auf etwas – hier steht, was es tut. **F1** hält den Text fest (Schloss-Symbol); noch einmal F1 auf demselben Element löst ihn wieder."],
  },
};

const ENTRIES: Entry[] = [
  // ------------------------------------------------ Leiste oben
  { sel: "kw-transport .brand", topic: { title: "Klangwelt", text: ["Musik machen, aufnehmen, erkennen und Klänge gestalten – alles in einem Programm. Die **Intro-Tour** auf der Seite eines leeren Projekts zeigt alle Fenster."] } },
  { sel: "[data-help=datei]", topic: { title: "Menü „Datei“", text: ["Neues Projekt, Öffnen, Speichern unter, Exportieren und das Demo-Projekt. Projekte liegen in Dokumente/Klangwelt/Projekte; eine .klangwelt-Datei enthält alles, auch die Aufnahmen."], keys: [["Strg+N", "neu"], ["Strg+O", "öffnen"], ["Strg+Umschalt+S", "speichern unter"]] } },
  { sel: ".save-btn", topic: { title: "Speichern", text: ["Speichert das Projekt in seine Datei; beim ersten Mal fragt Klangwelt nach einem Namen. Ein Punkt am Knopf und „•“ hinter dem Namen zeigen ungespeicherte Änderungen."], keys: [["Strg+S", "speichern"]] } },
  { sel: ".export-btn", topic: { title: "Exportieren", text: ["Schreibt das Stück als Datei – so, wie es zu hören ist (stumme Spuren fehlen, Solo gilt):", "**WAV** unkomprimiert · **FLAC** verlustfrei gepackt · **Opus** klein fürs Handy (Bitrate wählbar, Standard 160 kbit/s) · **Stems** eine WAV je Spur · **MIDI** nur die Noten.", "Ist ein Zeitraum markiert, kann der Export nur ihn schreiben. Ablage: Musik/Klangwelt/Exporte."], keys: [["Strg+E", "Export-Dialog"]] } },
  { sel: ".proj-name", topic: { title: "Projektname", text: ["Name der Projektdatei; „•“ heißt: ungespeicherte Änderungen. Beim Zeigen steht der volle Pfad im Tooltip."] } },
  { sel: "kw-transport .btn.big.rec", topic: { title: "Aufnehmen", text: ["Nimmt über das Mikrofon auf und legt die Aufnahme als Audio-Clip an. Danach kannst du sie analysieren, ihr Tempo erkennen oder die Melodie als Noten übernehmen."], keys: [["R", "Aufnahme starten / beenden"]] } },
  { sel: "kw-transport .btn.big.primary", topic: { title: "Wiedergabe", text: ["Spielt ab oder hält an. Ist in der Zeitleiste ein Zeitraum markiert, spielt sie genau diesen – mit Wiederholen in Schleife. Ohne Markierung stoppt sie einen Takt nach dem Ende des Stücks."], keys: [["Leertaste", "Wiedergabe / Pause"]] } },
  { sel: "kw-transport .btn.big", topic: { title: "Stopp", text: ["Hält an und setzt Abspielposition und Ansicht zurück an den Anfang."], keys: [["Enter / Pos1", "Stopp"]] } },
  { sel: "kw-transport .lcd", topic: { title: "Position", text: ["Oben Takt.Schlag.Sechzehntel, unten die Zeit in Minuten und Sekunden."] } },
  { sel: "[data-help=tempo]", topic: { title: "Tempo", text: ["Projekttempo in Schlägen pro Minute (BPM). Instrumentspuren folgen sofort; Audio-Clips mit „Synchronisieren“ werden gedehnt, ohne die Tonhöhe zu ändern."] } },
  { sel: "[data-help=takt]", topic: { title: "Takt", text: ["Taktart des Projekts (z. B. 4/4). Sie bestimmt Taktstriche, Raster und Metronom."] } },
  { sel: "[data-help=metronom]", topic: { title: "Metronom", text: ["Klick auf jedem Schlag, betont auf der Eins. Klangwelt merkt sich den Zustand bis zum nächsten Start."] } },
  { sel: "[data-help=wiederholen]", topic: { title: "Wiederholen", text: ["Spielt in Schleife: den markierten Zeitraum oder, ohne Markierung, das ganze Stück (auf ganze Takte aufgerundet)."], keys: [["L", "Wiederholen ein/aus"]] } },
  { sel: "[data-help=rueckgaengig]", topic: { title: "Rückgängig", text: ["Nimmt den letzten Schritt zurück – auch Änderungen des Agenten. Der Tooltip nennt den Schritt."], keys: [["Strg+Z", "rückgängig"]] } },
  { sel: "[data-help=wiederherstellen]", topic: { title: "Wiederherstellen", text: ["Holt einen zurückgenommenen Schritt wieder."], keys: [["Strg+Y / Strg+Umschalt+Z", "wiederherstellen"]] } },
  { sel: "[data-help=fenster]", topic: { title: "Fenster", text: ["Blendet Fenster ein und aus und setzt das Layout zurück. Fenster ziehst du an ihrem Reiter woandershin: andocken, als Reiter gruppieren oder frei schweben lassen."] } },
  { sel: "[data-help=einstellungen]", topic: { title: "Einstellungen", text: ["Grundeinstellungen, z. B. die Tonachse: tiefe Töne oben (wie an einem gedrehten Klavier) oder hohe Töne oben (wie in Noten)."] } },
  { sel: "canvas.meter", topic: { title: "Pegel", text: ["Oben der Ausgang, unten das Mikrofon. Rot heißt: zu laut – dann Pegel der Spuren senken."] } },
  { sel: ".chip.agent", topic: { title: "Agent", text: [
    "Zeigt, ob ein KI-Agent (z. B. Claude Code) mit Klangwelt verbunden ist:",
    "**Kein Agent verbunden** – der Endpunkt der App läuft und wartet; kein Agent hat sich in den letzten Minuten gemeldet oder hält eine Verbindung. **Agent bereit** – ein Agent ist verbunden und kann das Projekt steuern. **Agent hört zu** – er holt Aufträge aus dem Chat sofort ab. **Agent aus** – der Endpunkt konnte nicht starten.",
    "Ein Klick öffnet den Agent-Chat mit dem Prompt zum Verbinden: kopieren, in Claude Code einfügen, fertig.",
  ] } },
  { sel: "[data-help=ausgabegeraet]", topic: { title: "Ausgabegerät", text: [
    "Das Gerät, über das Klangwelt spielt (Lautsprecher, Kopfhörer, Audio-Interface), und seine Abtastrate.",
    "Ein Klick öffnet die Liste: **Systemstandard** folgt dem Standardgerät von Windows bzw. macOS, darunter stehen alle Geräte. Die Wahl wirkt sofort – eine laufende Wiedergabe spielt auf dem neuen Gerät weiter – und gilt auch beim nächsten Start. Ist das gewählte Gerät dann nicht angeschlossen, spielt Klangwelt über den Systemstandard und sagt kurz Bescheid.",
    "Ohne Gerät läuft Klangwelt ohne Ton weiter.",
  ] } },
  { sel: "[data-help=aufnahmegeraet]", topic: { title: "Aufnahmegerät", text: [
    "Das Mikrofon für Aufnahmen und das Vorsingen in der Bibliothek. Ein Klick öffnet die Liste: **Systemstandard** oder ein bestimmtes Gerät (z. B. ein USB-Mikrofon oder Audio-Interface).",
    "Die Wahl gilt ab der nächsten Aufnahme und auch nach einem Neustart; während einer Aufnahme lässt sie sich nicht ändern.",
  ] } },

  // ------------------------------------------------ Spuren
  { sel: ".harmony-toggle", topic: { title: "Harmonie in der Zeitleiste", text: ["Blendet im Lineal ein Band mit den Akkorden ein, gefärbt nach ihrer Stimmung (lösend, spannend, überraschend …). Rote Kanten markieren harmonische Brüche – tonartfremde Akkorde ohne Erklärung oder Tritonus-Sprünge. Das Band erscheint auch, solange du im Harmonie-Fenster arbeitest."] } },
  { sel: "[data-help=neue-audiospur]", topic: { title: "Audiospur", text: ["Neue Spur für Aufnahmen. Die Aufnahme startest du mit R oder dem roten Knopf oben."] } },
  { sel: "[data-help=neue-instrumentspur]", topic: { title: "Instrumentspur", text: ["Neue Spur mit Klangerzeuger und einem kurzen Muster: Zupfer, Orgel, Cembalo, Bass, Synth oder Schlagzeug. Den Klang gestaltest du im Fenster „Klang“."] } },
  { sel: ".tracks-head", topic: panelTopics.spuren },
  { sel: ".track-row input.text", topic: { title: "Name der Spur", text: ["Klicken und tippen benennt die Spur um; Enter übernimmt. Der Agent spricht Spuren mit Namen und Kennung an."] } },
  { sel: ".track-row .toggle.m", topic: { title: "Stumm (M)", text: ["Schaltet die Spur stumm. Stumme Spuren fehlen auch im Export; der Agent erfährt, welche stumm sind."] } },
  { sel: ".track-row .toggle.s", topic: { title: "Solo (S)", text: ["Nur Solo-Spuren sind zu hören – auch mehrere zugleich. Gilt auch für den Export."] } },
  { sel: ".track-row .toggle.convert", topic: { title: "In Noten umwandeln", text: ["Erscheint bei Audiospuren in der Tonhöhen-Ansicht: legt die erkannten Töne aller Clips der Spur als Melodie-Spur direkt darunter an – auf Sechzehntel gerastert, tiefe Lagen als Bass-Spur. Ein Rückgängig-Schritt.", "Für Schlagzeug-Aufnahmen: Rechtsklick auf den Clip → „In Rhythmus-Spur umwandeln“ (Bassdrum, Snare, Hi-Hat)."] } },
  { sel: ".track-row .toggle.v.x", topic: { title: "Ausdrucksspur", text: ["Blendet unter den Noten eine Spur für den Ausdruck ein: Anschlag, Vibrato, Helligkeit oder Biegung je Note. Werte ziehen oder über mehrere Noten malen; Umschalt zieht eine gerade Linie (Crescendo), Doppelklick setzt zurück."] } },
  { sel: ".track-row .toggle.v", topic: { title: "Ansicht", text: ["Schaltet zwischen kompakter Zeile und Notenansicht (Pianorolle) um. Audiospuren zeigen in der Notenansicht, was die Analyse erkennt."] } },
  { sel: ".track-row .stepper", topic: { title: "Tonhöhe", text: ["Verschiebt die Spur in Halbtönen (−24 … +24). Bei Audio bleibt das Tempo gleich; beim Export und im MIDI ist die Verschiebung eingerechnet."] } },
  { sel: "[data-help=pegel]", topic: { title: "Pegel", text: ["Lautstärke der Spur in dB (−60 … +6). Doppelklick setzt 0 dB."] } },
  { sel: "[data-help=panorama]", topic: { title: "Panorama", text: ["Position im Stereobild, von links nach rechts. Doppelklick setzt die Mitte."] } },
  { sel: ".track-row canvas.keys", topic: { title: "Klaviatur", text: ["Ein um 90° gedrehtes Klavier: Klick spielt einen Ton an. Die Tonachse (tiefe oder hohe Töne oben) stellst du im Zahnrad um."], keys: [["Mausrad", "Tonbereich verschieben"], ["Strg + Mausrad", "zoomen"], ["Doppelklick", "einpassen"]] } },
  { sel: ".track-row .expr-select", topic: { title: "Ausdrucksform", text: ["Was die Ausdrucksspur zeigt: Anschlag (Lautstärke), Vibrato, Helligkeit oder Biegung (Anschleifen)."] } },
  { sel: ".track-row", topic: { title: "Spurkopf", text: ["Name, Stumm/Solo, Ansicht, Pegel, Panorama und Tonhöhe einer Spur. Die Linie zwischen zwei Spuren ziehen ändert die Höhe (Doppelklick: Standard)."] } },
  { sel: "kw-timeline canvas", topic: (el, _x, y) => (y - el.getBoundingClientRect().top < RULER_H ? ruler : lanes) },
  { sel: ".empty-hint .song-start", topic: { title: "Mit Song starten", text: ["Legt ein ganzes Stück aus einer Vorlage an (Trance, Ambient, Deep House, Synthwave, Lo-Fi). Im Fenster „Song“ gestaltest du den Aufbau weiter."] } },
  { sel: ".empty-hint .beat-start", topic: { title: "Mit Beat starten", text: ["Legt eine Schlagzeug-Spur mit vier Takten eines Genre-Musters an und übernimmt Tempo und Klang. Im Fenster „Rhythmus“ bearbeitest du ihn weiter."] } },
  { sel: ".empty-hint .intro-btn", topic: { title: "Intro-Tour", text: ["Ein Rundgang durch alle Fenster: Schritt für Schritt hervorgehoben, mit Zetteln zu den wichtigsten Funktionen. Jederzeit mit Esc beenden."] } },
  { sel: ".empty-hint", topic: { title: "Leeres Projekt", text: ["Lade die Demo, starte mit einem Song oder Beat, lege eine Spur an oder nimm direkt auf. Die Intro-Tour zeigt dir alle Fenster."] } },
  { sel: "kw-timeline", topic: lanes },

  // ------------------------------------------------ Clip
  { sel: "kw-panel[tab=clip] .mark.dyn", topic: { title: "Dynamik", text: ["Setzt den Anschlag der gewählten Noten (ohne Auswahl: aller Noten im Clip): pp 32, p 48, mp 64, mf 80, f 100, ff 120."] } },
  { sel: "kw-panel[tab=clip] .mark", topic: { title: "Vortragszeichen", text: ["Crescendo/Decrescendo, Akzent (+25), Staccato (halbe Länge), Legato (bis zur nächsten Note), Vibrato, Heller/Dunkler, Anschleifen und Neutral – für die gewählten Noten oder alle."] } },
  { sel: "kw-panel[tab=clip] input[type=range]", topic: { title: "Ausdrucksregler", text: ["Zeigen den Mittelwert der gewählten Noten; Ziehen setzt alle auf den Wert (ein Rückgängig-Schritt)."] } },

  // ------------------------------------------------ Klang
  { sel: "kw-panel[tab=klang] .snd-kinds", topic: { title: "Instrument", text: ["Wechselt den Klangerzeuger der Spur. Die Noten bleiben, der Klang beginnt beim Startpunkt des Instruments."] } },
  { sel: "kw-panel[tab=klang] .snd-mark", topic: { title: "Startpunkt", text: ["Benannter Punkt im Feld – ein Klick setzt die Regler dieses Felds, die anderen Felder bleiben unberührt."] } },
  { sel: "kw-panel[tab=klang] .snd-field[data-field=charakter]", topic: { title: "Feld „Charakter“", text: ["Je Instrument eigene Achsen (beim Schlagzeug z. B. knackig → mächtig und schlicht → komplex). Ein Punkt hier setzt viele Regler des Instruments auf einmal."] } },
  { sel: "kw-panel[tab=klang] .snd-field[data-field=farbe]", topic: { title: "Feld „Klangfarbe“", text: ["Waagrecht dunkel → hell (Kippfilter), senkrecht glatt → rau (Flattern, Sättigung, Körnung)."] } },
  { sel: "kw-panel[tab=klang] .snd-field[data-field=raum]", topic: { title: "Feld „Raum“", text: ["Waagrecht schmal → breit (Stereo, die Tiefen bleiben mittig), senkrecht nah → fern (Hall; aus der Ferne leiser und dumpfer)."] } },
  { sel: "kw-panel[tab=klang] .snd-field", topic: { title: "Klangfeld", text: ["Punkt ziehen oder Startpunkt anklicken. Jedes Feld setzt nur seine eigenen Regler."] } },
  { sel: "kw-panel[tab=klang] .snd-play", topic: { title: "Anspielen", text: ["Was nach dem Loslassen eines Reglers kurz erklingt: nichts, ein Einzelton oder das Muster. Die Wahl bleibt."] } },
  { sel: "kw-panel[tab=klang] .snd-wave", topic: { title: "Zeitverlauf", text: ["Wellenform eines Testtons. Beim Synth zeichnest du hier den Verlauf: Lautstärke und Helligkeit als Kurven über die Zeit – Punkte ziehen, Doppelklick setzt oder löscht einen Punkt."] } },
  { sel: "kw-panel[tab=klang] .snd-fine", topic: { title: "Feinschliff", text: ["Alle Regler des Instruments einzeln. Rechts die Messwerte des Klangs (Helligkeit, Ausklang, Grundton …) – dieselben, die der Agent liest."] } },
  { sel: "kw-panel[tab=klang] .snd-lib", topic: { title: "Meine Klänge", text: ["Sichert die aktuelle Einstellung unter einem Namen – alle Regler, die Stellung in den Feldern, beim Synth der Verlauf. Die Klänge gelten in jedem Projekt; ein Klick lädt einen in die Spur. ✎ benennt um, der Papierkorb löscht beim zweiten Klick."] } },

  // ------------------------------------------------ Rhythmus
  { sel: ".rh-genre", topic: { title: "Genre", text: ["Typische Muster eines Stils mit Tempo, empfohlenem Swing und einem Tipp. Ein Klick zeigt die Muster des Genres."] } },
  { sel: ".rh-pattern", topic: { title: "Muster", text: ["Lädt das Muster ins Raster – als Entwurf oder, bei gewähltem Schlagzeug-Clip, als neuer Inhalt des Clips."] } },
  { sel: ".rh-step", topic: { title: "Schritt", text: ["Ein Klick setzt oder löscht einen Schlag. Die hellen Spalten sind die Zählzeiten 1 2 3 4. Betonungen setzt Klangwelt selbst (Backbeat laut, Ghost Notes leise)."] } },
  { sel: ".rh-label", topic: { title: "Trommel", text: ["Ein Klick spielt die Trommel an."] } },
  { sel: ".rh-swing", topic: { title: "Swing", text: ["Verschiebt jede zweite Sechzehntel nach hinten – von gerade bis stark geswingt."] } },
  { sel: ".rh-write", topic: { title: "In Spur schreiben", text: ["Legt den Entwurf als Clip an: hinter den letzten Clip einer Schlagzeug-Spur oder in eine neue Spur mit dem Klang des Genres."] } },

  // ------------------------------------------------ Song
  { sel: ".song-tpl", topic: { title: "Vorlage", text: ["Lädt Aufbau, Tempo, Tonart und Akkorde eines Stils in den Editor. Erst „Song erzeugen“ schreibt Spuren."] } },
  { sel: ".song-cell", topic: { title: "Zelle", text: ["Wie stark eine Rolle in einem Abschnitt spielt: aus → sparsam → voll. Sparsam heißt z. B. Arpeggio in Achteln statt Sechzehnteln."], keys: [["Klick", "mehr"], ["Umschalt + Klick", "weniger"]] } },
  { sel: ".song-chords", topic: { title: "Akkorde", text: ["Akkorde reihum mit deutschen Tonnamen wie im Quintenzirkel (a = a-Moll, C = C-Dur, B = B♭) oder Stufen (i, IV, V …). Daneben die Takte je Akkord."] } },
  { sel: ".song-write", topic: { title: "Song erzeugen", text: ["Schreibt je Rolle eine Spur (alle Trommeln zusammen in „Beat“), einen Clip je Abschnitt, setzt das Tempo und zoomt auf den ganzen Song. Erneut erzeugen ersetzt die zuletzt erzeugten Spuren – ein Rückgängig-Schritt holt sie zurück."] } },

  // ------------------------------------------------ Bibliothek
  { sel: ".lib-name", topic: { title: "Bibliotheken", text: ["Name der aktiven Bibliothek. Ein Klick öffnet die Verwaltung: wechseln, neu anlegen (z. B. zum Ausprobieren), umbenennen, löschen (nur den Index, nie Partituren)."] } },
  { sel: "[data-help=lib-herunterladen]", topic: { title: "Herunterladen", text: ["Öffnet den Katalog der Musikbibliotheken: PDMX (über 250 000 Partituren), OpenScore, KernScores mit Bach, Mozart, Beethoven und DCML mit Harmonieanalyse. Jedes Paket nennt Umfang und Lizenz; ein Klick lädt es herunter und übernimmt es in die aktive Bibliothek – im Hintergrund, auch über einen Neustart hinweg.", "Läuft schon ein Download, heißt der Knopf „Einreihen“: Das Paket kommt in die Warteschlange und startet danach von selbst – so lassen sich mehrere Pakete auf einmal beauftragen."] } },
  { sel: "[data-help=lib-warteschlange]", topic: { title: "Warteschlange", text: ["Beauftragte Pakete, die nach dem laufenden Auftrag geladen und übernommen werden – der Reihe nach, jedes in die Bibliothek, die beim Beauftragen aktiv war.", "Das × nimmt ein Paket heraus, „Alle herausnehmen“ leert die Liste. „Abbrechen“ stoppt nur den laufenden Auftrag; danach kommt das nächste dran. Beim Schließen der App geht die Warteschlange verloren."] } },
  { sel: "canvas.lib-ruler", topic: { title: "Zeitleiste der Partitur", text: ["Ziehen wandert in der Zeit, ein Klick setzt den Cursor, Umschalt/Strg+Ziehen markiert einen Abschnitt zum Anhören oder Übernehmen."] } },
  { sel: "canvas.lib-roll", topic: { title: "Noten der Partitur", text: ["Alle Stimmen der gewählten Partitur."], keys: [["Mausrad", "scrollen"], ["Strg + Mausrad", "zoomen"], ["mittlere Maustaste", "ziehen"]] } },

  // ------------------------------------------------ Harmonie
  { sel: "kw-panel[tab=harmonie] .hc-mode", topic: { title: "Einfügen oder Ersetzen", text: ["**Einfügen** schafft Platz: Alles ab der Stelle rückt in allen Spuren um die Länge der neuen Akkorde nach hinten. **Ersetzen** überschreibt das Nachfolgende in der Ziel-Spur."] } },
  { sel: "kw-panel[tab=harmonie] .hc-follow", topic: { title: "Folgen", text: ["Der Verlauf greift beim Abspielen und Cursor-Setzen die Akkorde an dieser Stelle auf – deine eigenen neuen Akkorde bleiben dahinter stehen."] } },
  { sel: "kw-panel[tab=harmonie] .hc-chip", topic: { title: "Verlauf", text: ["Die Akkorde bis hierher. Ein Klick spielt einen an; die letzten neuen Akkorde schreibt „In Spur schreiben“ in die Ziel-Spur."] } },
  { sel: "kw-panel[tab=harmonie] .hc-pick", topic: { title: "Fortsetzung", text: ["Ein Vorschlag mit Stufe, Funktion und Begründung. Klick spielt den Übergang, Doppelklick übernimmt ihn in den Verlauf. Der Punkt davor zeigt die Stimmung."] } },
  { sel: "kw-panel[tab=harmonie] .hc-legend", topic: { title: "Stimmungen", text: ["lösend (grün), spannend (orange), überraschend (pink), hell und dunkel – mit der Maus über einer Stimmung treten nur ihre Übergänge hervor."] } },
  { sel: "kw-panel[tab=harmonie] .hc-circle", topic: { title: "Quintenzirkel", text: ["Außen Dur, Mitte Moll, innen vermindert; die Akkorde der Tonart sind eingefärbt. Die besten Fortsetzungen tragen Rang und Pfeil."], keys: [["Klick", "Übergang vorspielen"], ["Doppelklick", "in den Verlauf übernehmen"]] } },

  // ------------------------------------------------ Kontextmenü
  { sel: ".ctx-menu [data-act^=raster]", topic: { title: "Am Takt ausrichten", text: ["Rückt die markierten Noten auf das Raster: 1/4, 1/8, 1/16 oder das Raster der Ansicht (feiner, je weiter hineingezoomt). Gerechnet wird im Takt des Stücks, auch wenn der Clip versetzt beginnt.", "„Am Takt ausrichten“ setzt nur die Anfänge, „Anfang und Länge“ auch die Enden – jede Note bleibt mindestens ein Raster lang."] } },
  { sel: ".ctx-menu", topic: { title: "Kontextmenü", text: ["Was sich mit dem Angeklickten tun lässt: Noten ausrichten und transponieren, eine Aufnahme in eine Melodie- oder Rhythmus-Spur umwandeln, den markierten Zeitraum aus allen Spuren löschen. Esc oder ein Klick daneben schließt es."] } },

  // ------------------------------------------------ Agent
  { sel: ".chat-input", topic: { title: "Auftrag", text: ["Schreib, was der Agent tun soll – z. B. „Mach die Kick tiefer“ oder „Schreib eine Basslinie für die markierten Takte“. Hörbare Spuren und der markierte Zeitraum gehen automatisch mit."], keys: [["Enter", "senden"], ["Umschalt + Enter", "neue Zeile"]] } },

  // ------------------------------------------------ Fenster allgemein
  { sel: ".dk-tab", topic: { title: "Fenster-Reiter", text: ["Ziehen verschiebt das Fenster: aufs Ziel-Kreuz eines anderen Fensters zum Andocken, auf eine Reiterleiste zum Gruppieren, irgendwohin sonst zum Schweben. Das × oder die mittlere Maustaste schließt es; „Fenster ▾“ holt es zurück."], keys: [["Esc beim Ziehen", "abbrechen"]] } },
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
  /** Tooltip eines tieferen Elements ohne eigenen Eintrag („Hier: …“). */
  tip: string | null;
  /** Element, für das die Hilfe gilt (zum Wiedererkennen beim Einfrieren). */
  el: Element;
}

function tooltipOf(el: Element): string {
  return (el.getAttribute("title") || el.getAttribute("aria-label") || "").trim();
}

/** Hilfe zu einem Element (mit Mausposition für Flächen wie die Zeitleiste). */
export function helpFor(target: Element | null, x = 0, y = 0): HelpView | null {
  let tip: string | null = null;
  for (let el: Element | null = target; el; el = el.parentElement) {
    const entry = ENTRIES.find((e) => el!.matches(e.sel));
    if (entry) {
      const topic = typeof entry.topic === "function" ? entry.topic(el, x, y) : entry.topic;
      const own = tooltipOf(el);
      // Tooltip eines tieferen Elements oder dynamischer Tooltip des Elements selbst
      const extra = tip ?? (own && !topic.text.some((t) => t.includes(own)) ? own : null);
      return { topic, tip: extra && extra !== topic.title ? extra : null, el };
    }
    if (!tip) {
      const t = tooltipOf(el);
      if (t) tip = t;
    }
  }
  return null;
}

export const HELP_COUNT = ENTRIES.length;
