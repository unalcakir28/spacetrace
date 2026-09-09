import type { Dictionary } from "./en";

export const de: Dictionary = {
  locale: {
    label: "Sprache",
    pick: "Sprache wählen",
    autoNotice: "Auf Deutsch angezeigt, passend zu Ihrem Browser.",
    dismiss: "Schließen",
  },

  nav: {
    desktop: "Desktop-App",
    cli: "Kommandozeile",
    hub: "Hub",
    download: "Download",
    guide: "Handbuch",
    github: "GitHub",
    menu: "Menü",
    skip: "Zum Inhalt springen",
  },

  common: {
    copy: "Kopieren",
    copied: "Kopiert",
    copyCommand: "Befehl kopieren",
    replay: "Erneut abspielen",
    download: "Herunterladen",
    free: "Frei",
    size: "Größe",
    file: "Datei",
    platform: "Plattform",
    notes: "Hinweise",
    version: "Version",
    latestBuild: "neuester Build",
    stableRelease: "stabile Version",
    notARelease: "aus main gebaut, keine veröffentlichte Version",
    checksums: "Prüfsummen",
    learnMore: "Mehr erfahren",
    sourceOnGitHub: "Quellcode auf GitHub",
    apacheCore: "Apache-2.0",
    proprietary: "Proprietär",
    loading: "Wird geladen…",
  },

  home: {
    title: "spacetrace — sehen, was die Platte füllt, und was sich verändert hat",
    description:
      "Eine Desktop-App, die zeigt, was Ihre Platten füllt, Momentaufnahmen aufbewahrt und sagt, welcher Ordner wirklich gewachsen ist. Mit einem Kommandozeilenwerkzeug und einem selbst gehosteten Dashboard für Server.",
    kicker: "macOS · Windows · Linux",
    headlineGiven: "Jeder Speicheranalysator zeigt, was auf der Platte liegt.",
    headlineSecond: "Dieser zeigt auch, was sich verändert hat.",
    lede: "Ein Laufwerk scannen und als lebende Treemap durchsehen. Das Ergebnis als Momentaufnahme aufbewahren, nächste Woche wiederkommen — und den einen Ordner bekommen, der wirklich gewachsen ist, nicht das Wurzelverzeichnis darüber.",
    ctaPrimary: "App herunterladen",
    ctaSecondary: "Oder die Kommandozeile nutzen",
    noAccount: "Kein Konto, keine Telemetrie, nichts wird irgendwohin geschickt.",

    demoTitle: "Das ist die App. Machen Sie eine Probefahrt.",
    demoLede:
      "Das Original zeichnet Zehntausende Kacheln auf ein Canvas. Diese Version führt dasselbe Squarified-Layout auf einem kleineren Baum aus — direkt auf der Seite.",

    diffTitle: "Ein Scan ist eine Fotografie. Zwei sind eine Antwort.",
    diffLede:
      "Einen Scan als Momentaufnahme speichern und dann vergleichen. Ordner, die eine Veränderung nur weitergeben, werden übersprungen; berichtet wird die erste Ebene, auf der sich das Wachstum wirklich verteilt.",
    diffPoint:
      "Es nennt den Ordner, an dem Sie etwas ändern können — nicht seine Vorfahren.",

    partsTitle: "Ein Kern, drei Zugänge",
    partsLede:
      "Scanner, Momentaufnahmen-Speicher, Vergleich und Treemap-Layout sind eine einzige Rust-Bibliothek. Eine Momentaufnahme, die eines davon schreibt, können die beiden anderen lesen.",

    trustTitle: "Zahlen, die Sie nachprüfen können",
    trustLede:
      "Ein Speicherwerkzeug, dessen Zahlen Sie mit Ihrer eigenen Shell nicht in Übereinstimmung bringen können, hat die Diskussion verloren. Das sind Testbedingungen im Repository, keine Versprechen.",

    ctaTitle: "Finden Sie heraus, was gewachsen ist, während Sie nicht hingesehen haben.",
    ctaLede:
      "Die Desktop-App ist kostenlos herunterzuladen. Scanner, Kommandozeilenwerkzeug und Server-Agent sind Open Source und bleiben es.",
  },

  demo: {
    zoomHint: "Doppelklick auf einen Ordner, um hineinzugehen",
    backHint: "Eine Ebene höher",
    rootLabel: "Persönlicher Ordner",
    basisLabel: "Maß",
    onDisk: "Auf der Platte",
    logical: "Logisch",
    basisHelp:
      "Auf der Platte sind die tatsächlich belegten Blöcke. Logisch ist die Länge, die jede Datei angibt. Sie widersprechen sich, und beide sind richtig.",
    sparseCallout:
      "Wechseln Sie auf Logisch und sehen Sie zu, wie das Platten-Image die Karte verschlingt. Es behauptet ein Terabyte und belegt neunzehn Gigabyte — deshalb ist Auf der Platte die Voreinstellung.",
    selected: "Ausgewählt",
    share: "Anteil an diesem Scan",
    kind: "Art",
    sparse: "sparse",
    empty: "Nichts ausgewählt",
    emptyHelp: "Auf eine Kachel klicken, um sie anzusehen.",
    entries: "Einträge",
    tapHint: "Ordner zweimal antippen, um hineinzugehen",
  },

  categories: {
    image: "Bild",
    video: "Video",
    audio: "Audio",
    document: "Dokument",
    archive: "Archiv",
    code: "Code",
    binary: "Programm",
    cache: "Cache",
    other: "Sonstiges",
    directory: "Ordner",
  },

  parts: {
    desktopName: "Desktop-App",
    desktopRole: "macOS · Windows · Linux",
    desktopPitch:
      "Eine navigierbare Treemap, nach Dateityp gefärbt — Ordnerliste, Karte und Inspektor sind sich einig, welches Maß sie zeigen.",
    desktopPoints: [
      "Einen laufenden Scan, eine gespeicherte Momentaufnahme oder einen Server über HTTP durchsehen",
      "Zwei Momentaufnahmen vergleichen und den gewachsenen Ordner sehen",
      "Einträge in den Papierkorb legen, ohne die Stelle zu verlieren",
      "Das Fenster bleibt während eines Scans benutzbar",
    ],
    desktopCta: "Was die App macht",

    cliName: "Kommandozeile",
    cliRole: "eine statische Binärdatei · plus ein Server-Agent",
    cliPitch:
      "Derselbe Kern — als skriptbarer Befehl und als Dienst, der nach Zeitplan scannt und über HTTP antwortet.",
    cliPoints: [
      "Scannen, aufbewahren, vergleichen, auflisten, für ncdu exportieren",
      "Jeden lesenden Befehl auf einen entfernten Agenten richten",
      "Läuft auf einem NAS, im Container, auf altem glibc",
      "Der Agent liest. Er löscht nie etwas.",
    ],
    cliCta: "Befehle und Einrichtung",

    hubName: "Hub",
    hubRole: "selbst gehostet · eine Binärdatei, eine SQLite-Datei",
    hubPitch:
      "Agenten schicken ihre Momentaufnahmen hierher. Der Hub bewahrt den Verlauf, rechnet aus, was wächst, und meldet sich, bevor eine Platte voll ist.",
    hubPoints: [
      "Flotten-Dashboard nach Dringlichkeit sortiert, nicht nach Hostname",
      "Wachstumsrate und Prognose „voll in N Tagen“",
      "Schwellenwertregeln, per Webhook zugestellt",
      "Berührt die überwachten Maschinen nie",
    ],
    hubCta: "Hub betreiben",
  },

  trust: {
    duTitle: "Summen stimmen exakt mit Ihrer Shell überein",
    duBody:
      "Logisch zählt nur Dateibytes und stimmt mit du -sb überein. Auf der Platte sind die tatsächlich belegten Blöcke einschließlich Verzeichnisblöcke und stimmt mit du -s --block-size=1 überein. Geprüft auf /usr mit 141k Dateien.",
    capacityTitle: "Frei von gesamt, niemals „% belegt“",
    capacityBody:
      "Die Kapazität wird so gemeldet, wie df sie meldet. In einem APFS-Container, einem btrfs-Subvolume oder Thin-LVM würde eine Belegt-Zahl die Geschwister mitzählen und df für denselben Mountpunkt widersprechen — deshalb wird sie nicht ausgegeben.",
    forecastTitle: "Die Prognose wird häufiger zurückgehalten als gezeigt",
    forecastBody:
      "„Voll in N Tagen“ braucht mindestens drei Momentaufnahmen über mindestens einen Tag, eine lineare Anpassung mit r² ≥ 0,5, eine tatsächlich gemessene Kapazität und eine Antwort innerhalb von zehn Jahren. Ein selbstsicher falsches Datum ist schlimmer als gar keines.",
    snapshotTitle: "Eine Momentaufnahme ist eine gewöhnliche SQLite-Datei",
    snapshotBody:
      "Kein Exportformat, kein proprietärer Blob. Was der Agent aufbewahrt, ist dasselbe, was über die Leitung geht und auf Ihrem Rechner landet: eine entfernte Momentaufnahme wird mit demselben Code durchgesehen wie eine lokale.",
    errorsTitle: "Fehler werden gezählt, nicht verschluckt",
    errorsBody:
      "Ein unlesbarer Pfad wird gemeldet und stichprobenartig gezeigt; der Scan läuft weiter. Ein abgebrochener Scan gibt überhaupt keinen Baum zurück, denn ein Teilbaum sieht vollständig aus und meldet eine Summe, die für keine Platte je gestimmt hat.",
    deleteTitle: "Der Agent kann nicht löschen",
    deleteBody:
      "Software, die Sie auf einem Server installieren, gewinnt Vertrauen dadurch, dass sie das Beängstigende nicht kann. Die Desktop-App kann Einträge in den Papierkorb legen — nur bei einem laufenden Scan der Maschine, an der Sie sitzen, und nur nach Rückfrage.",
  },

  desktop: {
    title: "Desktop-App — spacetrace",
    description:
      "Eine navigierbare Treemap davon, was Ihre Platten füllt, nach Dateityp gefärbt, mit Momentaufnahmen, entfernten Agenten und Vergleichen. macOS, Windows und Linux.",
    kicker: "Tauri und Rust · ein schlankes Fenster über einem nativen Scanner",
    headlineGiven: "Eine Karte Ihrer Platte.",
    headlineSecond: "Dann eine Karte davon, was sich verändert hat.",
    lede: "Einen Ordner scannen und als nach Dateityp gefärbte Treemap durchsehen. Oder eine Momentaufnahme von letzter Woche öffnen, oder eine, die auf einem Server liegt, und beide nebeneinander legen.",
    ctaPrimary: "App herunterladen",
    ctaSecondary: "Alle Dateien ansehen",

    windowCaption:
      "Aus dem Stylesheet der Anwendung gezeichnet statt als Bildschirmfoto — sie bleibt also richtig, solange die Palette gilt.",

    featuresTitle: "Wie sie sich benutzt",
    featuresLede:
      "Die App ist ein Fenster auf denselben Rust-Kern, den die Kommandozeile nutzt. Ihre eigene Aufgabe ist, nicht im Weg zu stehen.",

    f1Title: "Das Fenster wird Ihnen bei der Arbeit nie weggenommen",
    f1Body:
      "Ein Scan meldet sich in einer Leiste, nicht hinter einem Vorhang. Was offen war, bleibt offen und benutzbar und wird erst ersetzt, wenn der neue Scan fertig ist. Brechen Sie ihn ab oder lassen Sie ihn scheitern — der vorherige ist noch da.",
    f2Title: "Hineinzoomen, ohne die Stelle zu verlieren",
    f2Body:
      "Doppelklick auf einen Ordner in der Karte, einen aus der Brotkrumenleiste wählen oder das Kontextmenü einer Zeile nutzen. Rücktaste geht eine Ebene höher. Ein Klick in der Ordnerliste bewegt die Karte nie — Durchsehen kann Ihnen die Ansicht also nicht wegnehmen.",
    f3Title: "Der Fortschrittsbalken gibt zu, wenn er schätzt",
    f3Body:
      "Ein Dateisystemdurchlauf hat keinen Nenner. Die einzige ehrliche Schätzung ist das letzte Mal, als diese App denselben Ordner mit denselben Optionen gescannt hat — und genau die nimmt sie, wobei die Leiste sagt, woher die Zahl kommt. Beim ersten Scan gibt es Zähler und Bewegung, aber keine Prozentangabe.",
    f4Title: "Löschen lädt nichts neu",
    f4Body:
      "Einträge in den Papierkorb zu legen, ändert den offenen Baum an seiner Stelle und korrigiert die Summen darüber: aufgeklappte Ordner bleiben aufgeklappt, die Karte bleibt stehen und die Bildlaufposition bewegt sich nicht. Hundert Einträge sind derselbe Aufruf wie einer — mit einem echten „3 von 11“.",
    f5Title: "Der Papierkorb schafft keinen Platz, und die App sagt das",
    f5Body:
      "Auf jeder unterstützten Plattform liegt der Papierkorb im selben Dateisystem: der Ordner wird kleiner, die Platte nicht. Die Rückfrage sagt es, der Hinweis danach sagt es, und die Freiplatz-Anzeige lässt sich anklicken, um nach dem Leeren neu zu messen.",
    f6Title: "Größen sagen immer, welches Maß sie sind",
    f6Body:
      "Auf der Platte als Voreinstellung, mit einem beschrifteten Schalter in der Werkzeugleiste statt einer versteckten Einstellung. Sortierung, die Zahl daneben und die Farbe der Zeile kommen aus demselben Maß — „größte zuerst“ bedeutet also dasselbe wie die Zahl in der Zeile.",

    sourcesTitle: "Drei Quellen, ein Fenster",
    sourcesLede:
      "Der App ist es gleich, woher ein Baum kommt, denn eine Momentaufnahme ist dieselbe SQLite-Datei, wo auch immer sie geschrieben wurde.",
    s1Title: "Dieser Rechner",
    s1Body:
      "Ein laufender Scan. Die einzige Quelle, in der Eingriffe in Dateien erlaubt sind — weil es die einzige ist, in der die Pfade noch bedeuten, was sie sagen.",
    s2Title: "Eine gespeicherte Momentaufnahme",
    s2Body:
      "Aus derselben Datenbank, in die die Kommandozeile schreibt. Speichern Sie den offenen Scan, und er erscheint in spacetrace scans — und umgekehrt.",
    s3Title: "Ein entfernter Agent",
    s3Body:
      "Richten Sie die App auf URL und Token eines Agenten und lesen Sie dessen Momentaufnahmen über HTTP — ohne sich je auf dem Server anmelden zu müssen.",
    s4Title: "Zwei auf einmal",
    s4Body:
      "Vergleichen Sie zwei beliebige Momentaufnahmen derselben Wurzel und sehen Sie, welcher Ordner wirklich gewachsen ist — mit derselben Verursacher-Regel wie in der Kommandozeile.",

    safetyTitle: "Was sie nicht tut",
    safetyBody1:
      "Die App liest. Die einzige Ausnahme ist In den Papierkorb, und die ist vierfach gesichert: das Backend verweigert, wenn der offene Baum kein laufender Scan dieser Maschine ist, es verweigert die Scan-Wurzel selbst, es prüft erneut, ob jeder Pfad noch existiert, und das Fenster fragt vorher — und listet auf, was verschwinden wird, wenn es mehr als eines ist.",
    safetyBody2:
      "Nichts wird jemals endgültig gelöscht. Es geht in den Papierkorb des Systems, aus dem Sie es zurückholen können.",
    safetyBody3:
      "Eingriffe in Dateien sind deaktiviert, sobald eine Momentaufnahme geöffnet ist. Eine Momentaufnahme ist ein Bild der Vergangenheit oder einer anderen Maschine, und ein Pfad darin bedeutet vielleicht nicht mehr, was er sagt.",

    notYetTitle: "Noch nicht fertig",
    notYetLede: "Genannt, statt entdeckt zu werden:",
    notYet: [
      "Der schnelle MFT-Weg unter Windows — Scans sind dort ein gewöhnlicher Verzeichnisdurchlauf",
      "Ein Einführungsbildschirm für Full Disk Access auf macOS",
      "Eine Zeitleiste über die gesamte Geschichte eines Ziels statt zweier Momentaufnahmen",
      "Die Treemap-Leistung auf WebKitGTK, dem Linux-Renderer, ist nicht optimiert",
    ],
  },

  cli: {
    title: "Kommandozeile — spacetrace",
    description:
      "Das spacetrace-Kommandozeilenwerkzeug und der Server-Agent: scannen, aufbewahren, vergleichen und eine entfernte Maschine über HTTP lesen. Eine statische Binärdatei, Apache-2.0.",
    kicker: "Eine statische Binärdatei · keine Laufzeit zu installieren · Apache-2.0",
    headlineGiven: "Derselbe Kern, ohne Fenster.",
    headlineSecond: "Skriptbar — und auf einem Server zu Hause.",
    lede: "Alles, was die App kann, als Befehl für einen Cron-Job — dazu ein Agent, der nach Zeitplan scannt und über HTTP antwortet, damit auch eine Maschine, auf der Sie sich nie anmelden, sagen kann, was mit ihrer Platte passiert ist.",
    installTitle: "Installation",
    installNote:
      "Installiert spacetrace und spacetrace-agent nach /usr/local/bin. Absichtlich langweiliges POSIX-sh, weil es auch auf NAS-Firmware laufen muss, deren Shell busybox ist.",
    windowsNote:
      "Unter Windows das Archiv entpacken und spacetrace.exe in den PATH legen. Für das Kommandozeilenwerkzeug gibt es keinen Installer.",

    toursTitle: "Die vier Befehle, die Sie wirklich brauchen",
    tour1Title: "Einen Ordner ansehen",
    tour1Body: "Läuft ihn durch und gibt die Summen aus. Nichts wird gespeichert, nichts verändert.",
    tour2Title: "Das Ergebnis aufbewahren",
    tour2Body: "Speichert den Scan als Momentaufnahme in einer gewöhnlichen SQLite-Datei, auf Wunsch mit Bezeichnung.",
    tour3Title: "Fragen, was sich verändert hat",
    tour3Body: "Vergleicht die letzten beiden Momentaufnahmen einer Wurzel — oder die neueste mit der Platte, wie sie jetzt ist.",
    tour4Title: "Eine andere Maschine lesen",
    tour4Body: "Jeder lesende Befehl nimmt --remote. Dieselben Unterbefehle, nur woanders hin gerichtet.",

    agentTitle: "Der Agent",
    agentLede:
      "spacetrace-agent ist derselbe Code als Dienst. Er scannt die konfigurierten Wurzeln nach Zeitplan, bewahrt die Momentaufnahmen auf und liefert sie über HTTP aus. Eine statische Binärdatei — und er liest nur.",
    agentPoints: [
      "Cron-Zeitpläne mit fünf Feldern pro Wurzel, Aufbewahrung nach Anzahl",
      "Bearer-Token ist Pflicht; serve startet ohne eines nicht",
      "Lauscht standardmäßig nur auf Loopback — ein Inventar nach außen zu geben, muss eine bewusste Änderung sein",
      "/health braucht kein Token, damit ein Container-Healthcheck funktioniert, und verrät nichts weiter",
      "Unbekannte Konfigurationsschlüssel werden beim Start abgelehnt, nicht stillschweigend ignoriert",
    ],
    agentCta: "Vollständige Einrichtung des Agenten",

    dockerTitle: "Im Container",
    dockerNote:
      "Den Host schreibgeschützt einhängen und den scannen. Das Image enthält amd64 und arm64.",

    openTitle: "Open Source — und das bleibt so",
    openBody:
      "Scanner, Momentaufnahmen-Speicher, Vergleich und beide Binärdateien stehen unter Apache-2.0. Der Agent läuft auf Ihren Servern, also müssen Sie ihn lesen können. Desktop-App und Hub sind der kommerzielle Teil.",
  },

  hub: {
    title: "Hub — spacetrace",
    description:
      "Der selbst gehostete spacetrace-Hub: Agenten schicken Momentaufnahmen, der Hub bewahrt den Verlauf, rechnet aus, was wächst, und meldet sich, bevor eine Platte voll ist.",
    kicker: "Selbst gehostet · eine Binärdatei, eine SQLite-Datei · keine Telemetrie",
    headlineGiven: "Vierzig Maschinen, vierzig Platten.",
    headlineSecond: "Eine Seite, das Schlimmste zuerst.",
    lede: "Agenten scannen ihre eigenen Maschinen und schicken die Momentaufnahmen hierher. Der Hub bewahrt den Verlauf, rechnet aus, was wächst, und meldet sich, bevor eine Platte voll ist. Er berührt die überwachten Maschinen nie.",
    ctaBinaries: "Binärdateien",
    dashCaption:
      "Gezeichnet, kein Bildschirmfoto. Das echte Dashboard ist serverseitig gerendertes HTML — ein selbst gehostetes Werkzeug, das erst ein npm install verlangt, bevor es eine Seite zeigt, ist ein schlechteres Werkzeug.",
    withheldNote:
      "Eine leere Prognose ist eine Weigerung zu raten, keine fehlende Zahl. Die Seite des Ziels sagt, welche Bedingung nicht erfüllt war.",

    setupTitle: "Einrichten",
    setupLede:
      "Zwei Zugangsdaten, eine Konfigurationsdatei und eine Zeile bei jedem Agenten. Mehr ist nicht aufzusetzen.",
    step1Title: "Admin-Token und Konfiguration anlegen",
    step1Body:
      "Als hub.toml speichern, dann docker compose up -d — oder die Binärdatei direkt starten und die Konfiguration prüfen, bevor sie ausliefert.",
    step2Title: "Anmelden und ein Agenten-Token erstellen",
    step2Body:
      "Den Hub öffnen, mit dem Inhalt von admin-token anmelden und auf der Seite Agents ein Token erstellen. Oder direkt über die Kommandozeile.",
    step3Title: "Jeden Agenten darauf richten",
    step3Body:
      "Das in den Zeitplan des Agenten aufnehmen — die Flotte füllt sich von selbst. Nochmaliges Senden ist harmlos: eine Momentaufnahme mit gleichem Host, gleicher Wurzel und gleicher Startzeit wird übersprungen.",
    step4Title: "TLS davorsetzen",
    step4Body:
      "Bevor der Hub über Ihr Netz hinaus erreichbar wird, TLS in einem Reverse Proxy terminieren. Das Sitzungscookie ist HttpOnly und SameSite=Strict; das Secure-Flag kommt daher, dass über HTTPS ausgeliefert wird.",

    credsTitle: "Zwei Zugangsdaten, mit Absicht",
    credsBody1:
      "Ein Token, das in einer Konfigurationsdatei auf einem NAS liegt, darf nicht der Schlüssel zum Inventar der ganzen Flotte sein. Ein Agenten-Token ist deshalb kein Dashboard-Zugang — und umgekehrt gilt dasselbe.",
    credsBody2:
      "Agenten-Token werden als SHA-256-Hashes gespeichert; eine Kopie der Datenbank ist also kein Satz funktionierender Zugangsdaten. Ein Widerruf behält den Eintrag und macht das Token sofort unbrauchbar.",
    credsTested: "Beide Richtungen sind durch Tests abgedeckt.",
    credCol: "Zugangsdaten",
    credCan: "Kann",
    credCannot: "Kann nicht",
    agentToken: "Agenten-Token",
    adminToken: "Admin-Token",
    agentCan: "Eine Momentaufnahme senden",
    agentCannot: "Dashboard oder API lesen",
    adminCan: "Alles lesen, Token und Regeln verwalten",
    adminCannot: "Momentaufnahmen senden",

    forecastTitle: "Die Prognose ist die am stärksten zurückgehaltene Funktion",
    forecastLede:
      "Sie ist am ehesten falsch — also die, die am häufigsten die Antwort verweigert. „Voll in N Tagen“ erscheint nur, wenn all das gilt.",
    fc1Title: "Mindestens drei Momentaufnahmen über mindestens einen Tag",
    fc1Body:
      "Zwei Punkte ergeben immer eine perfekte Gerade. Ein Tag ist der kürzeste Zeitraum, in dem ein Tagesmuster überhaupt sichtbar werden kann.",
    fc2Title: "Eine lineare Anpassung mit r² ≥ 0,5",
    fc2Body:
      "Plattennutzung ist häufig nicht linear. Eine Log-Rotation oder eine einmalige Wiederherstellung erzeugen bereitwillig eine Gerade, deren Steigung nichts bedeutet.",
    fc3Title: "Eine tatsächlich gemessene Kapazität",
    fc3Body:
      "Ohne die echte Größe des Dateisystems gibt es nichts zu füllen. Der Hub errät sie nicht aus der größten Summe, die er gesehen hat.",
    fc4Title: "Eine Antwort innerhalb von zehn Jahren",
    fc4Body: "„Voll in 4.000 Tagen“ ist Arithmetik, keine Information.",
    fc5Title: "Wachstum und Prognose sind verschiedene Zahlen",
    fc5Body:
      "Das Wachstum wird am gescannten Ordner gemessen. Die Prognose rechnet den freien Platz des Dateisystems mit dieser Rate hoch. Die Seiten sagen, was was ist, statt beides zu einer beruhigenden Zahl zu verschmelzen.",

    alertsTitle: "Warnungen",
    alertsLede:
      "Schwellenwertregeln, per Webhook zugestellt. Drei Arten, jede mit einer Wartezeit, damit eine sich füllende Platte nicht zu einem Strom gleicher Nachrichten wird:",
    alerts: [
      "Freier Platz unter einem Prozentwert",
      "Wachstum über einer Rate",
      "Eine Prognose, die in einen bestimmten Zeitraum fällt",
    ],
    alertsMissing:
      "Zustellung per E-Mail ist noch nicht gebaut, und es gibt einen Admin-Zugang statt Konten pro Person.",

    apiTitle: "HTTP-API",
    noRollupTitle: "Warum es keine Aggregat-Tabelle gibt",
    noRollupBody1:
      "Momentaufnahmen liegen unverändert in demselben Speicher, den Kommandozeile und Agent nutzen. Der Hub führt keine eigene Zusammenfassungstabelle — es gibt also nichts, was von den Momentaufnahmen abdriften könnte, aus denen es stammt.",
    noRollupBody2:
      "Was ein Agent schickt, ist genau die Datei, die er lokal aufbewahrt: auf keiner Seite wird umgewandelt, und der Vergleich auf der Seite eines Ziels wird von demselben Code berechnet, der auf Ihrem Rechner läuft.",

    fleetHost: "Host",
    fleetRoot: "Wurzel",
    fleetFree: "Frei",
    fleetGrowth: "Wachstum",
    fleetTrend: "Letzte 14 Tage",
    fleetFills: "Voll in",
    fleetDays: "Tagen",
    fleetTargets: "Ziele",
    fleetReporting: "melden sich",
    fleetSilent: "seit 3 Tagen still",
    fleetNoCapacity: "Kapazität nicht erfasst",
    fleetTwoSnapshots: "2 Momentaufnahmen",
  },

  download: {
    title: "spacetrace herunterladen",
    description:
      "Downloads und Installationsanleitungen für die spacetrace-Desktop-App, das Kommandozeilenwerkzeug mit Agent und den selbst gehosteten Hub. macOS, Windows, Linux, NAS und Docker.",
    headline: "Download",
    lede: "Drei Dinge zum Installieren, alle auf demselben Kern gebaut. Nehmen Sie, was Sie brauchen; eine Momentaufnahme von einem davon können die anderen lesen.",

    detectMac: "macOS",
    detectMacSub: "Universal — Apple Silicon und Intel",
    detectWindows: "Windows",
    detectWindowsSub: "64 Bit",
    detectLinux: "Linux",
    detectLinuxSub: "Statische Binärdateien — keine Laufzeit zu installieren",
    detectUnknown: "Plattform wählen",
    detectUnknownSub:
      "Dieser Browser hat nicht gesagt, auf welcher Maschine er läuft. Die Tabellen unten listen jede Datei.",
    goToFiles: "Zu den Dateilisten",
    alsoCli: "Kommandozeilenwerkzeug und Agent",
    cliAlongside:
      "App und Kommandozeilenwerkzeug teilen eine Momentaufnahmen-Datenbank und lassen sich daher zusammen benutzen.",

    continuousTitle: "Was „continuous“ bedeutet",
    continuousBody:
      "Bis zur ersten getaggten Version ist jeder Download hier der neueste Build von main: er hat die CI bestanden und sonst nichts. Sobald es eine stabile Version gibt, wechselt diese Seite automatisch dorthin.",

    desktopSection: "Desktop-App",
    desktopSectionLede:
      "Das Treemap-Fenster. Kostenlos herunterzuladen, solange sie in Phase 3 ist.",
    desktopCaption:
      "Der macOS-Build ist universal — es gibt also eine Datei für Apple Silicon und Intel.",

    cliSection: "Kommandozeile und Agent",
    cliSectionLede:
      "Ein Archiv enthält beide Binärdateien: spacetrace für die Kommandozeile und spacetrace-agent für den Dienst, der nach Zeitplan scannt und über HTTP antwortet.",
    cliCaption:
      "Die Linux-Builds sind statisch gegen musl gebaut, laufen also auf altem glibc und in einem Scratch-Container. Jedes Archiv enthält auch README und Lizenz.",

    hubSection: "Hub",
    hubSectionLede:
      "Die Flottenansicht. Eine Binärdatei, eine SQLite-Datei, kein Datenbankserver und kein Frontend-Build.",
    hubCaption:
      "Das Container-Image enthält beide Architekturen, docker pull holt also die richtige ohne Tag-Zusatz.",

    unsignedTitle: "Nichts hiervon ist signiert",
    unsignedBody:
      "Signaturzertifikate kosten jährlich Geld und können nicht in einem öffentlichen Repository liegen — macOS und Windows halten Sie also beide beim ersten Mal auf. Das verdient Misstrauen, und genau deshalb steht neben jeder Datei eine Prüfsumme und der Build ist ein Workflow, den Sie lesen können.",

    installMacTitle: "macOS: „spacetrace kann nicht geöffnet werden“",
    installMacBody:
      "Das .dmg öffnen, die App in Programme ziehen, dann Rechtsklick und Öffnen wählen — das gibt die einmalige Erlaubnis, die ein Doppelklick nicht anbietet. Oder das Quarantäne-Attribut direkt entfernen:",
    installMacFda:
      "Für Scans außerhalb Ihres persönlichen Ordners braucht die App Full Disk Access, unter Systemeinstellungen → Datenschutz & Sicherheit.",
    installWinTitle: "Windows: SmartScreen",
    installWinBody:
      "„Der Computer wurde durch Windows geschützt“ → Weitere Informationen → Trotzdem ausführen. Der Installer läuft pro Benutzer und braucht keine Administratorrechte.",
    installLinuxTitle: "Linux",
    installLinuxBody:
      "Das AppImage braucht keine Installation: ausführbar machen und starten. Bleibt das Fenster unter Wayland leer, mit WEBKIT_DISABLE_DMABUF_RENDERER=1 starten — das ist ein WebKitGTK-Problem, keines der App.",

    verifyTitle: "Prüfen, was Sie heruntergeladen haben",
    verifyBody:
      "Jede Version bringt eine SHA256SUMS-Datei mit, die ihre eigenen Dateien abdeckt. Prüfen Sie vor dem Installieren — zumal keine der Binärdateien signiert ist.",
    verifyMacNote: "macOS hat kein sha256sum; nehmen Sie shasum -a 256 -c.",

    buildTitle: "Oder selbst bauen",
    buildBody:
      "Kern, Kommandozeilenwerkzeug und Agent stehen unter Apache-2.0 und brauchen nur Rust 1.85 oder neuer.",
    buildNote:
      "Desktop-App und Hub sind nicht Open Source und lassen sich daher nicht aus dem Quellcode bauen — das ist der Preis dafür, dass der Kern unter einer permissiven Lizenz bleibt.",

    platMacArm: "macOS, Apple Silicon",
    platMacIntel: "macOS, Intel",
    platMacAny: "macOS 11+",
    platWin: "Windows 10 / 11",
    platLinuxX64: "Linux, x86_64",
    platLinuxArm: "Linux, aarch64",
    platDebian: "Debian, Ubuntu",
    platFedora: "Fedora, RHEL",
    platAnyLinux: "Beliebiges Linux",
    noteUniversal: "Universal-Binärdatei",
    noteNsis: "NSIS-Installer, pro Benutzer",
    noteDeb: "Benötigt libwebkit2gtk-4.1",
    noteRpm: "Benötigt webkit2gtk4.1",
    noteAppImage: "Ausführbar machen und starten; nichts wird installiert",
    noteStatic: "Statisch; Server und Container",
    noteNas: "Raspberry Pi, ARM-Server, die meisten NAS-Geräte",
    noteM1: "M1 und neuer",
    noteMac11: "macOS 11+",
    noteWinAlloc:
      "Hier entspricht die Größe auf der Platte der logischen Größe; siehe bekannte Grenzen",
    noteAllArchives: "Eine Datei für alle Archive darüber",
    noteHubServer: "Statisch; die übliche Wahl für einen Server",
    noteHubTry: "Um es erst auf dem eigenen Rechner auszuprobieren",
  },

  guide: {
    title: "Handbuch — spacetrace",
    description:
      "Wie man spacetrace benutzt: scannen, Momentaufnahmen aufbewahren, zwei davon vergleichen, den Agenten auf einem Server einrichten, eine entfernte Maschine lesen — und was welches Größenmaß bedeutet.",
    headline: "Handbuch",
    lede: "Alles, was das Kommandozeilenwerkzeug kann, in der Reihenfolge, in der Sie es wahrscheinlich brauchen. --help bei jedem Befehl gibt dasselbe ohne Browser aus.",
    onThisPage: "Auf dieser Seite",

    startTitle: "Erster Scan",
    startBody:
      "Ein Scan läuft einen Ordner durch und gibt aus, was er gefunden hat. Nichts wird gespeichert, nichts verändert.",
    startUnreadable:
      "Unlesbare Pfade werden gezählt und stichprobenartig gezeigt, statt den Durchlauf abzubrechen. Unter macOS braucht der Weg aus Ihrem persönlichen Ordner heraus Full Disk Access für das Terminal; ohne das läuft der Scan trotzdem durch und sagt, was er nicht gesehen hat.",
    startNarrowTitle: "Schneller und enger machen",
    startExclude:
      "--exclude nimmt einen Verzeichnisnamen, keinen Pfad, und lässt sich wiederholen. In ausgeschlossene Ordner wird nie hineingegangen: node_modules auf einer Entwicklungsmaschine auszuschließen ist meist der Unterschied zwischen zwei und dreißig Sekunden.",

    snapTitle: "Momentaufnahmen und Vergleich",
    snapBody:
      "Das ist der Teil, den andere Analysatoren nicht haben. Einen Scan speichern, später noch einen, und fragen, was sich dazwischen verändert hat.",
    snapThen: "Dann vergleichen. Ohne Argumente nimmt es die letzten beiden.",
    culpritTitle: "Warum es den tiefsten Ordner nennt",
    culpritBody:
      "Ein Ordner, dessen Wachstum vollständig von einem einzigen Kind kommt, sagt Ihnen nichts Neues und wird deshalb übersprungen. Der Bericht nennt die erste Ebene, auf der sich die Veränderung wirklich verteilt — den Verursacher, nicht seine Vorfahren.",
    snapSinceTitle: "Gegen die Platte, wie sie jetzt ist",
    snapSinceBody:
      "Sie brauchen keine zwei Momentaufnahmen. Vergleichen Sie die neueste mit einem laufenden Scan:",
    snapFilterTitle: "Auswählen, was erscheint",
    snapPruneTitle: "Die Datenbank nicht unendlich wachsen lassen",

    browseTitle: "Einen Scan durchsehen",
    browseBody:
      "ls listet Ordner nach Größe — aus einem laufenden Scan oder aus einer gespeicherten Momentaufnahme.",
    browseExport:
      "Um eine Momentaufnahme an etwas anderes zu geben, schreibt export das Format von ncdu:",
    browseJson:
      "Jeder Befehl nimmt außerdem --json, und das ist der unterstützte Weg, darauf zu skripten. Die menschenlesbaren Spalten dürfen sich ändern, die Form des JSON nicht.",

    agentTitle: "Der Agent auf einem Server",
    agentBody:
      "spacetrace-agent ist derselbe Code als Dienst: er scannt die konfigurierten Wurzeln nach Zeitplan, bewahrt die Momentaufnahmen auf und antwortet über HTTP. Eine statische Binärdatei — und er liest nur; es gibt in ihm keinen Codeweg, der außerhalb seiner eigenen Momentaufnahmen-Datenbank etwas löscht.",
    agentStep1: "Konfiguration und Token schreiben",
    agentStep1Body:
      "serve startet ohne Token nicht. Ein Agent ohne Authentifizierung übergibt sein gesamtes Dateisystem-Inventar an jeden, der den Port erreicht.",
    agentStep2: "Sagen, was wann gescannt wird",
    agentStep2Body:
      "Fünf Cron-Felder, mit *, a-b, */n und Listen. Keine Sekunden, kein @daily. Unbekannte Konfigurationsschlüssel werden beim Start abgelehnt, denn ein Tippfehler, der auf einer unbeobachteten Maschine stillschweigend nichts tut, ist schlimmer als eine Startverweigerung.",
    agentStep3: "Vor dem Start prüfen",
    agentStep3Body:
      "Ein Zeitplan, der nie auslösen kann, meldet never statt still zu scheitern.",
    agentStep4: "Starten",
    agentStep4Body:
      "Die mitgelieferte systemd-Unit führt den Agenten als eigenen unprivilegierten Benutzer mit ProtectSystem=strict aus, mit Nice=10 und Idle-E/A-Priorität. Ein Scan darf nie dem in den Weg kommen, wofür die Maschine eigentlich da ist.",
    agentDocker: "In Docker den Host schreibgeschützt einhängen und den scannen:",
    agentSecTitle: "Bevor Sie ihn nach außen geben",
    agentSec: [
      "Er lauscht standardmäßig auf Loopback. Ein Dateisystem-Inventar in ein Netz zu stellen, muss eine bewusste Änderung sein.",
      "Im Agenten gibt es kein TLS. Setzen Sie einen Reverse Proxy davor.",
      "Das Token wird ohne vorzeitigen Abbruch verglichen: ein falsches Token braucht gleich lange zur Ablehnung, egal wie viel davon richtig war.",
      "/health braucht kein Token, damit ein Container-Healthcheck funktioniert, und gibt nur Status und Version zurück — keinen Hostnamen, keine Wurzeln.",
      "Scans auf Zuruf sind aus. Eingeschaltet kann jeder mit dem Token jedes Verzeichnis auflisten, das der Benutzer des Agenten lesen darf.",
    ],

    remoteTitle: "Eine andere Maschine lesen",
    remoteBody:
      "Jeder lesende Befehl nimmt --remote. Es ist nichts Neues zu lernen: dieselben Unterbefehle, nur woanders hin gerichtet.",
    remoteSaveBody:
      "Speichern Sie ein Ziel, damit URL und Token nicht mehr getippt werden müssen — in ~/.config/spacetrace/remotes.toml:",
    remoteNotDifferentTitle: "Eine entfernte Momentaufnahme ist nichts anderes",
    remoteNotDifferentBody:
      "Was über die Leitung kommt, ist dieselbe eigenständige SQLite-Datei, die der Agent aufbewahrt: auflisten, durchsehen und vergleichen läuft mit genau demselben Code wie bei einer lokalen. Sie können das Werkzeug auch ganz weglassen und trotzdem eine Datei bekommen, die Sie öffnen können:",
    remoteRefuse:
      "scan, prune und rm verweigern sich mit --remote: sie arbeiten auf lokalem Zustand, und der Agent löscht nichts.",

    sizeTitle: "Welche Größe, und warum",
    sizeBody: "Jeder Eintrag trägt zwei Zahlen, und keine ist eine Schätzung der anderen.",
    sizeMeasure: "Maß",
    sizeWhat: "Was es ist",
    sizeMatches: "Stimmt überein mit",
    sizeLogical: "logisch",
    sizeLogicalWhat: "Die Länge, die jede Datei angibt",
    sizeOnDisk: "auf der Platte",
    sizeOnDiskWhat: "Tatsächlich belegte Blöcke, Verzeichnisblöcke inbegriffen",
    sizeCapacity: "Kapazität",
    sizeCapacityWhat: "Freier Platz von gesamt, auf dem gescannten Dateisystem",
    sizeCapacityMatches: "der Avail-Spalte von df, exakt",
    sizeDiverge:
      "Sie weichen in beide Richtungen ab, und beide sind richtig. Eine Ein-Byte-Datei belegt einen ganzen Block, ist auf der Platte also größer als ihre Länge. Eine Sparse-Datei gibt eine Länge an, die sie nie belegt hat: ein Platten-Image kann ein Terabyte behaupten und neunzehn Gigabyte belegen.",
    sizeWhyOnDisk:
      "Sparse-Dateien sind der Grund, warum die App auf der Platte voreinstellt. Images virtueller Maschinen, Datenbankdateien und Core-Dumps gehören auf jeder echten Platte zu den größten Einträgen — das logische Maß irrt sich also genau bei den Einträgen am stärksten, die am meisten zählen. Die Kommandozeile stellt logisch voreingestellt, und beide sagen, welches sie zeigen.",
    sizeCapacityWhy:
      "Die Kapazität wird als frei von gesamt gemeldet, nie als „% belegt“. Auf einem Dateisystem, dessen Platz zwischen Volumes geteilt wird — APFS-Container, btrfs-Subvolumes, Thin-LVM — würde eine Belegt-Zahl die Geschwister mitzählen und df für denselben Mountpunkt widersprechen.",
    sizeLinks:
      "Hardlinks werden standardmäßig einmal gezählt; die zweite Kopie erscheint im Baum mit einem Beitrag von null Byte. Symlinks werden nie verfolgt und zählen mit ihrer eigenen Größe.",

    refTitle: "Befehlsübersicht",
    refOptionsTitle: "Optionen, die bei den meisten Befehlen gelten",

    storageTitle: "Wo was liegt",
    storagePlatform: "Plattform",
    storageDb: "Momentaufnahmen-Datenbank",
    storageAnywhere: "Überall",
    storageOverride:
      "SPACETRACE_HOME überschreibt das; --db überschreibt es für einen einzelnen Befehl",
    storageShared:
      "Die Desktop-App schreibt in dieselbe Datenbank: eine in der App gespeicherte Momentaufnahme erscheint in spacetrace scans — und umgekehrt.",

    limitsTitle: "Bekannte Grenzen",
    limitsLede:
      "Veröffentlicht, statt entdeckt zu werden. Wenn Sie auf eine davon stoßen, ist es bekannte Schuld, keine Überraschung.",
    limits: [
      "Unter Windows entspricht die Größe auf der Platte der logischen Größe, und die Hardlink-Deduplizierung ist aus. Echte Zahlen brauchen GetFileInformationByHandleEx und FileIdInfo.",
      "APFS-Klone werden nicht dedupliziert, und auf btrfs oder ZFS führen Reflinks und Kompression dazu, dass ein Baumdurchlauf die echte Nutzung nicht melden kann. Kein Baumdurchlauf kann das; die Zahlen sind die der Dateien selbst.",
      "Ein Scan hält den ganzen Baum im Speicher. Das Speicherprofil jenseits von zehn Millionen Dateien ist nicht gemessen.",
      "Der Agent hat kein TLS und keine Ratenbegrenzung. Nutzen Sie einen Reverse Proxy; das Token ist ohnehin Pflicht.",
      "Der Scheduler hat keine Zeitzonendatenbank — UTC plus ein fester Offset, Sommerzeitwechsel bleiben also Ihre Sache.",
    ],
  },

  footer: {
    licence:
      "Scanner, Momentaufnahmen-Speicher, Kommandozeilenwerkzeug und Agent stehen unter Apache-2.0. Desktop-App und Hub sind proprietär.",
    home: "Start",
    allReleases: "Alle Versionen",
    builtWith: "Mit Astro gebaut. Keine Tracker, keine Cookies.",
  },
};
