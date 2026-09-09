import type { Dictionary } from "./en";

export const it: Dictionary = {
  locale: { label: "Lingua", pick: "Scegli una lingua" },

  nav: {
    desktop: "App desktop",
    cli: "Riga di comando",
    hub: "Hub",
    download: "Scarica",
    guide: "Guida",
    github: "GitHub",
    menu: "Menu",
    skip: "Vai al contenuto",
  },

  common: {
    copy: "Copia",
    copied: "Copiato",
    copyCommand: "Copia il comando",
    replay: "Riproduci",
    download: "Scarica",
    free: "Libero",
    size: "Dimensione",
    file: "File",
    platform: "Piattaforma",
    notes: "Note",
    version: "Versione",
    latestBuild: "ultima build",
    stableRelease: "release stabile",
    notARelease: "compilata da main, non è una release",
    checksums: "Checksum",
    learnMore: "Scopri di più",
    sourceOnGitHub: "Codice su GitHub",
    apacheCore: "Apache-2.0",
    proprietary: "Proprietario",
    loading: "Caricamento…",
  },

  home: {
    title: "spacetrace — vedi cosa riempie il disco e cosa è cambiato",
    description:
      "Un'app desktop che mappa cosa sta riempiendo i tuoi dischi, conserva istantanee e ti dice quale cartella è davvero cresciuta. Con uno strumento a riga di comando e una dashboard self-hosted per i server.",
    kicker: "macOS · Windows · Linux",
    headlineGiven: "Ogni analizzatore di dischi mappa cosa c'è sul disco.",
    headlineSecond: "Questo mostra anche cosa è cambiato.",
    lede: "Scansiona un'unità ed esplorala come una treemap dal vivo. Conserva il risultato come istantanea, torna la settimana dopo e ottieni l'unica cartella che è davvero cresciuta — non la radice sotto cui si trova.",
    ctaPrimary: "Scarica l'app",
    ctaSecondary: "Oppure usa la riga di comando",
    noAccount: "Nessun account, nessuna telemetria, nessun caricamento di nulla.",

    demoTitle: "Questa è l'app. Fai un giro di prova.",
    demoLede:
      "Quella vera disegna decine di migliaia di riquadri su un canvas. Questa esegue lo stesso layout squarified su un albero più piccolo, direttamente in pagina.",

    diffTitle: "Una scansione è una fotografia. Due sono una risposta.",
    diffLede:
      "Salva una scansione come istantanea, poi confronta. Le cartelle che si limitano a trasmettere una variazione vengono saltate, e viene riportato il primo livello in cui la crescita si distribuisce davvero.",
    diffPoint:
      "Nomina la cartella su cui puoi agire, non i suoi antenati.",

    partsTitle: "Un solo nucleo, tre modi di entrarci",
    partsLede:
      "Lo scanner, l'archivio delle istantanee, il confronto e il layout della treemap sono un'unica libreria Rust. Un'istantanea scritta da uno di questi è leggibile dagli altri due.",

    trustTitle: "Numeri che puoi verificare",
    trustLede:
      "Uno strumento per dischi che stampa una cifra non riconciliabile con la tua shell ha già perso la discussione. Queste sono condizioni di test nel repository, non promesse.",

    ctaTitle: "Scopri cosa è cresciuto mentre non guardavi.",
    ctaLede:
      "L'app desktop è scaricabile gratuitamente. Lo scanner, lo strumento a riga di comando e l'agente per server sono open source e lo resteranno.",
  },

  demo: {
    zoomHint: "Doppio clic su una cartella per entrarci",
    backHint: "Torna su",
    rootLabel: "Home",
    basisLabel: "Misura",
    onDisk: "Su disco",
    logical: "Logica",
    basisHelp:
      "Su disco sono i blocchi effettivamente allocati. Logica è la lunghezza che ogni file dichiara. Non concordano, ed entrambe sono corrette.",
    sparseCallout:
      "Passa a Logica e guarda l'immagine disco divorare la mappa. Dichiara un terabyte e ne occupa diciannove gigabyte: ecco perché l'app usa su disco come impostazione predefinita.",
    selected: "Selezionato",
    share: "Quota di questa scansione",
    kind: "Tipo",
    sparse: "sparso",
    empty: "Nessuna selezione",
    emptyHelp: "Fai clic su un riquadro per ispezionarlo.",
    entries: "voci",
    tapHint: "Tocca due volte una cartella per entrarci",
  },

  categories: {
    image: "immagine",
    video: "video",
    audio: "audio",
    document: "documento",
    archive: "archivio",
    code: "codice",
    binary: "binario",
    cache: "cache",
    other: "altro",
    directory: "cartella",
  },

  parts: {
    desktopName: "App desktop",
    desktopRole: "macOS · Windows · Linux",
    desktopPitch:
      "Una treemap navigabile colorata per tipo di file, in cui l'elenco delle cartelle, la mappa e l'ispettore concordano su quale misura stanno mostrando.",
    desktopPoints: [
      "Esplora una scansione dal vivo, un'istantanea salvata o un server via HTTP",
      "Confronta due istantanee e vedi la cartella che è cresciuta",
      "Sposta voci nel Cestino senza perdere il punto in cui eri",
      "La finestra resta usabile mentre una scansione è in corso",
    ],
    desktopCta: "Cosa fa l'app",

    cliName: "Riga di comando",
    cliRole: "un binario statico · più un agente per server",
    cliPitch:
      "Lo stesso nucleo, come comando da script e come servizio che scansiona su pianificazione e risponde via HTTP.",
    cliPoints: [
      "Scansiona, salva istantanee, confronta, elenca, esporta per ncdu",
      "Punta qualsiasi comando di sola lettura verso un agente remoto",
      "Funziona su un NAS, in un container, su glibc datate",
      "L'agente legge. Non cancella mai nulla.",
    ],
    cliCta: "Comandi e configurazione",

    hubName: "Hub",
    hubRole: "self-hosted · un binario, un file SQLite",
    hubPitch:
      "Gli agenti gli inviano le loro istantanee. L'hub conserva la cronologia, calcola cosa sta crescendo e ti avverte prima che un disco si riempia.",
    hubPoints: [
      "Dashboard della flotta ordinata per urgenza, non per nome host",
      "Tasso di crescita e previsione “si riempie in N giorni”",
      "Regole di soglia consegnate via webhook",
      "Non tocca mai le macchine che osserva",
    ],
    hubCta: "Avvia l'hub",
  },

  trust: {
    duTitle: "I totali coincidono esattamente con la tua shell",
    duBody:
      "Logica conta solo i byte dei file e coincide con du -sb. Su disco sono i blocchi effettivamente allocati, inclusi quelli delle directory, e coincide con du -s --block-size=1. Verificato su /usr con 141k file.",
    capacityTitle: "Libero sul totale, mai “% usato”",
    capacityBody:
      "La capacità è riportata come la riporta df. Su un container APFS, un subvolume btrfs o un LVM thin, una cifra di spazio usato includerebbe i fratelli e contraddirebbe df sullo stesso mount — quindi non viene stampata.",
    forecastTitle: "La previsione viene taciuta più spesso di quanto sia mostrata",
    forecastBody:
      "“Si riempie in N giorni” richiede almeno tre istantanee su almeno un giorno, un fit lineare con r² ≥ 0,5, una capacità davvero misurata e una risposta entro dieci anni. Una data sbagliata ma sicura è peggio di nessuna data.",
    snapshotTitle: "Un'istantanea è un semplice file SQLite",
    snapshotBody:
      "Non un formato di esportazione, non un blob proprietario. Ciò che l'agente conserva è ciò che viaggia in rete ed è ciò che arriva sul tuo portatile: un'istantanea remota si esplora con lo stesso codice di una locale.",
    errorsTitle: "Gli errori si contano, non si ingoiano",
    errorsBody:
      "Un percorso illeggibile viene segnalato e campionato; la scansione continua. Una scansione annullata non restituisce alcun albero, perché un albero parziale sembra completo e riporta un totale che non è mai stato vero per nessun disco.",
    deleteTitle: "L'agente non ha la cancellazione",
    deleteBody:
      "Il software che installi su un server guadagna fiducia non potendo fare la cosa spaventosa. L'app desktop può spostare voci nel Cestino — solo su una scansione dal vivo della macchina davanti a te, e solo dopo aver chiesto.",
  },

  desktop: {
    title: "App desktop — spacetrace",
    description:
      "Una treemap navigabile di cosa sta riempiendo i tuoi dischi, colorata per tipo di file, con istantanee, agenti remoti e confronti. macOS, Windows e Linux.",
    kicker: "Tauri e Rust · una finestra leggera su uno scanner nativo",
    headlineGiven: "Una mappa del tuo disco.",
    headlineSecond: "Poi una mappa di cosa è cambiato.",
    lede: "Scansiona una cartella ed esplorala come treemap colorata per tipo di file. Oppure apri un'istantanea della settimana scorsa, o una che sta su un server, e mettile a confronto.",
    ctaPrimary: "Scarica l'app",
    ctaSecondary: "Vedi tutti i file",

    windowCaption:
      "Disegnata dal foglio di stile dell'applicazione invece di essere uno screenshot, così resta fedele fintanto che la palette non cambia.",

    featuresTitle: "Com'è usarla",
    featuresLede:
      "L'app è una finestra sullo stesso nucleo Rust usato dalla riga di comando. Il suo compito è togliersi di mezzo.",

    f1Title: "La finestra non ti viene mai portata via mentre lavora",
    f1Body:
      "Una scansione si annuncia in una striscia, non dietro una tendina. Ciò che era aperto resta aperto e usabile, e viene sostituito solo quando la nuova scansione finisce. Fermala, o lasciala fallire, e la precedente è ancora lì.",
    f2Title: "Entra nel dettaglio senza perdere il punto",
    f2Body:
      "Doppio clic su una cartella nella mappa, scegline una dal percorso, o usa il menu contestuale di una riga. Backspace risale. Un clic nell'elenco delle cartelle non muove mai la mappa, così esplorare non può farti perdere la vista.",
    f3Title: "La barra di avanzamento ammette quando sta indovinando",
    f3Body:
      "Percorrere un filesystem non ha un denominatore. L'unica stima onesta è l'ultima volta che questa app ha scansionato la stessa cartella con le stesse opzioni, ed è quella che usa — e la striscia dice da dove viene il numero. Alla prima scansione ci sono contatori e movimento, ma nessuna percentuale.",
    f4Title: "Cancellare non ricarica nulla",
    f4Body:
      "Spostare voci nel Cestino modifica l'albero aperto sul posto e corregge i totali sopra di esso: le cartelle espanse restano espanse, la mappa resta dov'è e lo scorrimento non si muove. Cento voci sono la stessa chiamata di una, con un vero “3 di 11”.",
    f5Title: "Spostare nel Cestino non libera spazio, e l'app lo dice",
    f5Body:
      "Su ogni piattaforma supportata il Cestino è sullo stesso filesystem: la cartella si rimpicciolisce, il disco no. Lo dice la conferma, lo dice l'avviso successivo, e si può fare clic sullo spazio libero per rimisurarlo dopo aver svuotato.",
    f6Title: "Le dimensioni dicono sempre quale misura sono",
    f6Body:
      "Su disco per impostazione predefinita, con un interruttore etichettato nella barra degli strumenti anziché un'opzione sepolta. L'ordinamento, la cifra accanto e il colore della riga vengono dalla stessa misura, così “prima i più grandi” significa la stessa cosa del numero stampato sulla riga.",

    sourcesTitle: "Tre sorgenti, una finestra",
    sourcesLede:
      "All'app non importa da dove venga un albero, perché un'istantanea è lo stesso file SQLite in qualunque punto sia stata scritta.",
    s1Title: "Questa macchina",
    s1Body:
      "Una scansione dal vivo. L'unica sorgente in cui è permesso agire sui file, perché è l'unica in cui i percorsi significano ancora ciò che dicono.",
    s2Title: "Un'istantanea salvata",
    s2Body:
      "Dallo stesso database che scrive la riga di comando. Salva la scansione aperta e comparirà in spacetrace scans, e viceversa.",
    s3Title: "Un agente remoto",
    s3Body:
      "Punta l'app all'URL e al token di un agente e leggi le sue istantanee via HTTP — senza dover mai accedere al server.",
    s4Title: "Due alla volta",
    s4Body:
      "Confronta due istantanee qualsiasi della stessa radice e vedi quale cartella è davvero cresciuta, con la stessa regola della cartella colpevole usata dalla riga di comando.",

    safetyTitle: "Cosa non farà",
    safetyBody1:
      "L'app legge. L'unica eccezione è Sposta nel Cestino, protetta in quattro modi: il backend rifiuta se l'albero aperto non è una scansione dal vivo di questa macchina, rifiuta la radice stessa della scansione, ricontrolla che ogni percorso esista ancora, e la finestra chiede prima — elencando cosa sta per andarsene quando è più di una cosa.",
    safetyBody2:
      "Niente viene mai cancellato direttamente. Va nel Cestino di sistema, da cui puoi rimetterlo a posto.",
    safetyBody3:
      "Agire sui file è disattivato quando è aperta un'istantanea. Un'istantanea è la fotografia del passato, o di un'altra macchina, e un percorso al suo interno potrebbe non significare più ciò che dice.",

    notYetTitle: "Non ancora fatto",
    notYetLede: "Dichiarato invece di essere scoperto:",
    notYet: [
      "Il percorso rapido MFT su Windows — lì le scansioni sono un normale attraversamento di directory",
      "Una schermata di onboarding per il Full Disk Access su macOS",
      "Una vista cronologica dell'intera storia di un target, invece di due istantanee alla volta",
      "Le prestazioni della treemap su WebKitGTK, il renderer di Linux, non sono state ottimizzate",
    ],
  },

  cli: {
    title: "Riga di comando — spacetrace",
    description:
      "Lo strumento a riga di comando spacetrace e l'agente per server: scansiona, salva istantanee, confronta e leggi una macchina remota via HTTP. Un binario statico, Apache-2.0.",
    kicker: "Un binario statico · nessun runtime da installare · Apache-2.0",
    headlineGiven: "Lo stesso nucleo, senza finestra.",
    headlineSecond: "Scriptabile, e a casa su un server.",
    lede: "Tutto ciò che fa l'app, come comando da mettere in un cron job — più un agente che scansiona su pianificazione e risponde via HTTP, così anche una macchina a cui non accedi mai può dirti cosa è successo al suo disco.",
    installTitle: "Installazione",
    installNote:
      "Installa spacetrace e spacetrace-agent in /usr/local/bin. Deliberatamente noioso sh POSIX, perché deve girare anche sul firmware di un NAS la cui shell è busybox.",
    windowsNote:
      "Su Windows, estrai l'archivio e metti spacetrace.exe nel PATH. Per lo strumento a riga di comando non c'è un installer.",

    toursTitle: "I quattro comandi che userai davvero",
    tour1Title: "Guarda una cartella",
    tour1Body: "La percorre e stampa i totali. Niente viene salvato, niente viene modificato.",
    tour2Title: "Conserva il risultato",
    tour2Body: "Salva la scansione come istantanea in un semplice file SQLite, con un'etichetta se vuoi.",
    tour3Title: "Chiedi cosa è cambiato",
    tour3Body: "Confronta le ultime due istantanee di una radice, o la più recente con il disco così com'è ora.",
    tour4Title: "Leggi un'altra macchina",
    tour4Body: "Qualsiasi comando di sola lettura accetta --remote. Stessi sottocomandi, puntati altrove.",

    agentTitle: "L'agente",
    agentLede:
      "spacetrace-agent è lo stesso codice come servizio. Scansiona su pianificazione le radici che configuri, conserva le istantanee e le serve via HTTP. Un binario statico — e legge soltanto.",
    agentPoints: [
      "Pianificazioni cron a cinque campi per radice, con ritenzione per numero",
      "Token bearer obbligatorio; serve rifiuta di partire senza uno",
      "Ascolta su loopback per impostazione predefinita — esporre un inventario deve essere una scelta deliberata",
      "/health non richiede token, così un healthcheck del container funziona, e non rivela nient'altro",
      "Le chiavi di configurazione sconosciute vengono rifiutate all'avvio anziché ignorate in silenzio",
    ],
    agentCta: "Configurazione completa dell'agente",

    dockerTitle: "In un container",
    dockerNote:
      "Monta l'host in sola lettura e scansiona quello. L'immagine contiene sia amd64 sia arm64.",

    openTitle: "Open source, e così resterà",
    openBody:
      "Lo scanner, l'archivio delle istantanee, il confronto ed entrambi i binari sono Apache-2.0. L'agente gira sui tuoi server, quindi devi poterlo leggere. L'app desktop e l'hub sono la parte commerciale.",
  },

  hub: {
    title: "Hub — spacetrace",
    description:
      "L'hub spacetrace self-hosted: gli agenti inviano istantanee, l'hub conserva la cronologia, calcola cosa sta crescendo e ti avverte prima che un disco si riempia.",
    kicker: "Self-hosted · un binario, un file SQLite · nessuna telemetria",
    headlineGiven: "Quaranta macchine, quaranta dischi.",
    headlineSecond: "Una pagina, prima i peggiori.",
    lede: "Gli agenti scansionano le proprie macchine e inviano qui le istantanee. L'hub conserva la cronologia, calcola cosa sta crescendo e ti avverte prima che un disco si riempia. Non tocca mai le macchine che osserva.",
    ctaBinaries: "Binari",
    dashCaption:
      "Disegnata, non uno screenshot. La dashboard reale è HTML generato dal server: uno strumento self-hosted che ha bisogno di un npm install prima di mostrarti una pagina è uno strumento peggiore.",
    withheldNote:
      "Una previsione vuota è un rifiuto di indovinare, non un numero mancante. La pagina del target dice quale condizione non era soddisfatta.",

    setupTitle: "Come configurarlo",
    setupLede:
      "Due credenziali, un file di configurazione e una riga aggiunta a ogni agente. Non c'è altro da mettere in piedi.",
    step1Title: "Crea un token di amministrazione e una configurazione",
    step1Body:
      "Salvala come hub.toml, poi docker compose up -d — oppure esegui il binario direttamente e valida la configurazione prima di servire.",
    step2Title: "Accedi e crea un token per l'agente",
    step2Body:
      "Apri l'hub, accedi con il contenuto di admin-token e crea un token nella pagina Agents. O fallo dalla riga di comando.",
    step3Title: "Punta ogni agente verso di lui",
    step3Body:
      "Aggiungilo alla pianificazione dell'agente e la flotta si popola da sola. Rinviare è innocuo: un'istantanea già presente con lo stesso host, radice e ora di inizio viene saltata.",
    step4Title: "Mettici davanti il TLS",
    step4Body:
      "Prima di esporre l'hub fuori dalla tua rete, termina il TLS in un reverse proxy. Il cookie di sessione è HttpOnly e SameSite=Strict; il flag Secure arriva servendolo via HTTPS.",

    credsTitle: "Due credenziali, di proposito",
    credsBody1:
      "Un token che sta in un file di configurazione su un NAS non deve essere la chiave dell'inventario di tutta la flotta. Per questo un token di agente non è una credenziale per la dashboard — e vale anche il contrario.",
    credsBody2:
      "I token degli agenti sono conservati come hash SHA-256, quindi una copia del database non è un insieme di credenziali funzionanti. Revocare conserva la traccia e blocca il token immediatamente.",
    credsTested: "Entrambe le direzioni sono coperte da test.",
    credCol: "Credenziale",
    credCan: "Può",
    credCannot: "Non può",
    agentToken: "Token agente",
    adminToken: "Token amministratore",
    agentCan: "Inviare un'istantanea",
    agentCannot: "Leggere la dashboard o l'API",
    adminCan: "Leggere tutto, gestire token e regole",
    adminCannot: "Inviare istantanee",

    forecastTitle: "La previsione è la funzione trattenuta più a lungo",
    forecastLede:
      "È quella con più probabilità di essere sbagliata, quindi è quella che più spesso si rifiuta di rispondere. “Si riempie in N giorni” compare solo se valgono tutte queste condizioni.",
    fc1Title: "Almeno tre istantanee, su almeno un giorno",
    fc1Body:
      "Due punti formano sempre una retta perfetta. Un giorno è l'intervallo più breve in cui un andamento quotidiano possa emergere.",
    fc2Title: "Un fit lineare con r² ≥ 0,5",
    fc2Body:
      "L'uso del disco spesso non è lineare. Una rotazione dei log o un ripristino occasionale producono volentieri una retta la cui pendenza non significa nulla.",
    fc3Title: "Una capacità davvero misurata",
    fc3Body:
      "Senza la dimensione reale del filesystem non c'è nulla da riempire. L'hub non la indovina dal totale più grande che ha visto.",
    fc4Title: "Una risposta entro dieci anni",
    fc4Body: "“Si riempie in 4.000 giorni” è aritmetica, non informazione.",
    fc5Title: "Crescita e previsione sono numeri diversi",
    fc5Body:
      "La crescita è misurata sulla cartella scansionata. La previsione proietta lo spazio restante del filesystem a quel ritmo. Le pagine dicono quale è quale, invece di fonderli in un'unica cifra rassicurante.",

    alertsTitle: "Avvisi",
    alertsLede:
      "Regole di soglia, consegnate via webhook. Tre tipi, ognuno con un periodo di attesa perché un disco che si riempie non diventi un flusso di messaggi identici:",
    alerts: [
      "Spazio libero sotto una percentuale",
      "Crescita sopra un certo ritmo",
      "Una previsione che cade entro un orizzonte",
    ],
    alertsMissing:
      "La consegna via email non è ancora implementata, e c'è una sola credenziale di amministrazione invece di accessi per persona.",

    apiTitle: "API HTTP",
    noRollupTitle: "Perché non esiste una tabella di riepilogo",
    noRollupBody1:
      "Le istantanee sono conservate senza modifiche nello stesso archivio usato dalla riga di comando e dall'agente. L'hub non tiene una propria tabella di riepilogo, quindi non c'è nulla che possa disallinearsi dalle istantanee da cui deriva.",
    noRollupBody2:
      "Ciò che un agente invia è lo stesso file che conserva localmente: niente viene convertito da nessuna delle due parti, e il confronto nella pagina di un target è calcolato dallo stesso codice che gira sul tuo portatile.",

    fleetHost: "Host",
    fleetRoot: "Radice",
    fleetFree: "Libero",
    fleetGrowth: "Crescita",
    fleetTrend: "Ultimi 14 giorni",
    fleetFills: "Si riempie in",
    fleetDays: "giorni",
    fleetTargets: "target",
    fleetReporting: "attivi",
    fleetSilent: "silenzioso da 3 giorni",
    fleetNoCapacity: "capacità non registrata",
    fleetTwoSnapshots: "2 istantanee",
  },

  download: {
    title: "Scarica spacetrace",
    description:
      "Download e istruzioni di installazione per l'app desktop spacetrace, lo strumento a riga di comando con agente e l'hub self-hosted. macOS, Windows, Linux, NAS e Docker.",
    headline: "Scarica",
    lede: "Tre cose installabili, tutte costruite sullo stesso nucleo. Prendi quella che ti serve; un'istantanea scritta da una qualsiasi è leggibile dalle altre.",

    detectMac: "macOS",
    detectMacSub: "Universal — Apple silicon e Intel",
    detectWindows: "Windows",
    detectWindowsSub: "64 bit",
    detectLinux: "Linux",
    detectLinuxSub: "Binari statici — nessun runtime da installare",
    detectUnknown: "Scegli una piattaforma",
    detectUnknownSub:
      "Questo browser non ha detto su quale macchina si trova. Le tabelle qui sotto elencano ogni file.",
    goToFiles: "Vai agli elenchi dei file",
    alsoCli: "Strumento a riga di comando e agente",
    cliAlongside:
      "L'app e lo strumento a riga di comando condividono un unico database di istantanee, quindi possono essere usati insieme.",

    continuousTitle: "Cosa significa “continuous”",
    continuousBody:
      "Fino alla prima release con tag, ogni download qui è la build più recente di main: ha superato la CI e nient'altro. Quando esisterà una release stabile, questa pagina passerà automaticamente a quella.",

    desktopSection: "App desktop",
    desktopSectionLede:
      "La finestra con la treemap. Scaricabile gratuitamente finché è in fase 3.",
    desktopCaption:
      "La build macOS è universal, quindi c'è un unico file per Apple silicon e Intel.",

    cliSection: "Riga di comando e agente",
    cliSectionLede:
      "Un solo archivio contiene entrambi i binari: spacetrace per la riga di comando e spacetrace-agent per il servizio che scansiona su pianificazione e risponde via HTTP.",
    cliCaption:
      "Le build Linux sono musl statiche, quindi girano su glibc vecchie e dentro un container scratch. Ogni archivio contiene anche il README e la licenza.",

    hubSection: "Hub",
    hubSectionLede:
      "La vista della flotta. Un binario, un file SQLite, nessun server di database e nessuna build frontend.",
    hubCaption:
      "L'immagine del container contiene entrambe le architetture, quindi docker pull prende quella giusta senza suffissi nel tag.",

    unsignedTitle: "Niente qui è firmato digitalmente",
    unsignedBody:
      "I certificati di firma costano ogni anno e non possono stare in un repository pubblico, quindi macOS e Windows ti fermeranno entrambi la prima volta. È giusto essere sospettosi, ed è per questo che accanto a ogni file c'è un checksum e la build è un workflow che puoi leggere.",

    installMacTitle: "macOS: “impossibile aprire spacetrace”",
    installMacBody:
      "Apri il .dmg, trascina l'app in Applicazioni, poi fai clic destro e scegli Apri — è quello a dare il permesso una volta sola che il doppio clic non offre. Oppure rimuovi direttamente il flag di quarantena:",
    installMacFda:
      "Scansionare fuori dalla tua cartella home richiede il Full Disk Access per l'app, in Impostazioni di Sistema → Privacy e sicurezza.",
    installWinTitle: "Windows: SmartScreen",
    installWinBody:
      "“Windows ha protetto il PC” → Ulteriori informazioni → Esegui comunque. L'installer è per utente e non richiede diritti di amministratore.",
    installLinuxTitle: "Linux",
    installLinuxBody:
      "L'AppImage non richiede alcuna installazione: rendila eseguibile ed eseguila. Su Wayland, se la finestra resta bianca, esegui con WEBKIT_DISABLE_DMABUF_RENDERER=1 — è un problema di WebKitGTK, non dell'app.",

    verifyTitle: "Verifica ciò che hai scaricato",
    verifyBody:
      "Ogni release porta un file SHA256SUMS che copre i propri asset. Verifica prima di installare, tanto più che nessuno dei binari è firmato.",
    verifyMacNote: "macOS non ha sha256sum; usa shasum -a 256 -c.",

    buildTitle: "Oppure compilalo tu",
    buildBody:
      "Il nucleo, lo strumento a riga di comando e l'agente sono Apache-2.0 e richiedono solo Rust 1.85 o successivo.",
    buildNote:
      "L'app desktop e l'hub non sono open source, quindi non possono essere compilati dal codice — è il compromesso perché il nucleo resti con una licenza permissiva.",

    platMacArm: "macOS, Apple silicon",
    platMacIntel: "macOS, Intel",
    platMacAny: "macOS 11+",
    platWin: "Windows 10 / 11",
    platLinuxX64: "Linux, x86_64",
    platLinuxArm: "Linux, aarch64",
    platDebian: "Debian, Ubuntu",
    platFedora: "Fedora, RHEL",
    platAnyLinux: "Qualsiasi Linux",
    noteUniversal: "Binario universal",
    noteNsis: "Installer NSIS, per utente",
    noteDeb: "Richiede libwebkit2gtk-4.1",
    noteRpm: "Richiede webkit2gtk4.1",
    noteAppImage: "Rendila eseguibile ed esegui; non viene installato nulla",
    noteStatic: "Statico; server e container",
    noteNas: "Raspberry Pi, server ARM, la maggior parte dei NAS",
    noteM1: "M1 e successivi",
    noteMac11: "macOS 11+",
    noteWinAlloc:
      "Qui la dimensione su disco è pari a quella logica; vedi i limiti noti",
    noteAllArchives: "Un solo file che copre tutti gli archivi qui sopra",
    noteHubServer: "Statico; la scelta abituale per un server",
    noteHubTry: "Per provarlo prima sulla tua macchina",
  },

  guide: {
    title: "Guida — spacetrace",
    description:
      "Come usare spacetrace: scansionare, conservare istantanee, confrontarne due, configurare l'agente su un server, leggere una macchina remota e cosa significa ciascuna misura di dimensione.",
    headline: "Guida",
    lede: "Tutto ciò che fa lo strumento a riga di comando, nell'ordine in cui probabilmente ti servirà. --help su qualsiasi comando stampa la stessa cosa senza browser.",
    onThisPage: "In questa pagina",

    startTitle: "Prima scansione",
    startBody:
      "Una scansione percorre una cartella e stampa ciò che ha trovato. Niente viene salvato, niente viene modificato.",
    startUnreadable:
      "I percorsi illeggibili vengono contati e campionati invece di fermare il percorso. Su macOS, andare fuori dalla cartella home richiede il Full Disk Access per il terminale; senza, la scansione si completa comunque e ti dice cosa ha mancato.",
    startNarrowTitle: "Renderla più veloce e più ristretta",
    startExclude:
      "--exclude prende il nome di una directory, non un percorso, e si ripete. Nelle cartelle escluse non si scende mai: escludere node_modules su una macchina da sviluppo è di solito la differenza fra due secondi e trenta.",

    snapTitle: "Istantanee e confronto",
    snapBody:
      "Questa è la parte che gli altri analizzatori non hanno. Salva una scansione, salvane un'altra più tardi e chiedi cosa è cambiato.",
    snapThen: "Poi confronta. Senza argomenti prende le ultime due.",
    culpritTitle: "Perché nomina la cartella più profonda",
    culpritBody:
      "Una cartella la cui crescita viene interamente da un solo figlio non ti dice nulla di nuovo, quindi viene saltata. Il report nomina il primo livello in cui la variazione si distribuisce davvero — il colpevole, non i suoi antenati.",
    snapSinceTitle: "Contro il disco così com'è ora",
    snapSinceBody:
      "Non ti servono due istantanee. Confronta la più recente con una scansione dal vivo:",
    snapFilterTitle: "Scegliere cosa compare",
    snapPruneTitle: "Evitare che il database cresca all'infinito",

    browseTitle: "Esplorare una scansione",
    browseBody:
      "ls elenca le cartelle per dimensione, da una scansione dal vivo o da un'istantanea salvata.",
    browseExport:
      "Per passare un'istantanea a qualcos'altro, export scrive il formato di ncdu:",
    browseJson:
      "Ogni comando accetta anche --json, che è il modo supportato per scriptarci sopra. Le colonne leggibili possono cambiare; la forma del JSON no.",

    agentTitle: "L'agente su un server",
    agentBody:
      "spacetrace-agent è lo stesso codice come servizio: scansiona su pianificazione le radici che configuri, conserva le istantanee e risponde via HTTP. È un binario statico, e legge soltanto — al suo interno non esiste alcun percorso di codice che cancelli qualcosa fuori dal proprio database di istantanee.",
    agentStep1: "Scrivi una configurazione e un token",
    agentStep1Body:
      "serve rifiuta di partire senza un token. Un agente senza autenticazione consegna l'intero inventario del filesystem a chiunque riesca a raggiungere la porta.",
    agentStep2: "Di' cosa scansionare e quando",
    agentStep2Body:
      "Cinque campi cron, con *, a-b, */n ed elenchi. Nessun secondo, nessun @daily. Le chiavi di configurazione sconosciute vengono rifiutate all'avvio, perché un errore di battitura che non fa nulla in silenzio su una macchina che nessuno guarda è peggio di un rifiuto di avviarsi.",
    agentStep3: "Controlla prima di avviarlo",
    agentStep3Body:
      "Una pianificazione che non può mai scattare riporta never invece di fallire in silenzio.",
    agentStep4: "Avvialo",
    agentStep4Body:
      "L'unità systemd inclusa esegue l'agente come utente non privilegiato con ProtectSystem=strict, a Nice=10 e con priorità I/O idle. Una scansione non deve mai mettersi fra la macchina e ciò per cui esiste.",
    agentDocker: "In Docker, monta l'host in sola lettura e scansiona quello:",
    agentSecTitle: "Prima di esporlo",
    agentSec: [
      "Ascolta su loopback per impostazione predefinita. Pubblicare l'inventario di un filesystem su una rete deve essere una scelta deliberata.",
      "Nell'agente non c'è TLS. Mettici davanti un reverse proxy.",
      "Il token è confrontato senza uscita anticipata, quindi un token sbagliato richiede lo stesso tempo per essere rifiutato, per quanto ne fosse corretto.",
      "/health non richiede token, così un healthcheck del container funziona, e restituisce solo stato e versione — nessun hostname, nessuna radice.",
      "Le scansioni su richiesta sono disattivate. Con quelle attive, chi ha il token può elencare qualsiasi directory leggibile dall'utente dell'agente.",
    ],

    remoteTitle: "Leggere un'altra macchina",
    remoteBody:
      "Qualsiasi comando di sola lettura accetta --remote. Non c'è nulla di nuovo da imparare: gli stessi sottocomandi, puntati altrove.",
    remoteSaveBody:
      "Salva un remoto così URL e token smettono di essere digitati — in ~/.config/spacetrace/remotes.toml:",
    remoteNotDifferentTitle: "Un'istantanea remota non è una cosa diversa",
    remoteNotDifferentBody:
      "Ciò che arriva dalla rete è lo stesso file SQLite autonomo che l'agente conserva: elencarla, esplorarla e confrontarla esegue lo stesso codice di un'istantanea locale. Puoi anche saltare lo strumento e ottenere comunque un file apribile:",
    remoteRefuse:
      "scan, prune e rm rifiutano di girare con --remote: agiscono sullo stato locale, e l'agente non cancella nulla.",

    sizeTitle: "Quale dimensione, e perché",
    sizeBody: "Ogni voce porta due numeri, e nessuno dei due è una stima dell'altro.",
    sizeMeasure: "Misura",
    sizeWhat: "Che cos'è",
    sizeMatches: "Coincide con",
    sizeLogical: "logica",
    sizeLogicalWhat: "La lunghezza dichiarata da ogni file",
    sizeOnDisk: "su disco",
    sizeOnDiskWhat: "Blocchi effettivamente allocati, inclusi quelli delle directory",
    sizeCapacity: "capacità",
    sizeCapacityWhat: "Spazio libero sul totale, nel filesystem scansionato",
    sizeCapacityMatches: "la colonna Avail di df, esattamente",
    sizeDiverge:
      "Divergono in entrambe le direzioni ed entrambe sono corrette. Un file di un byte alloca un blocco intero, quindi su disco è più grande della sua lunghezza. Un file sparso dichiara una lunghezza che non ha mai allocato: un'immagine disco può dichiarare un terabyte e occuparne diciannove gigabyte.",
    sizeWhyOnDisk:
      "I file sparsi sono il motivo per cui l'app usa su disco per impostazione predefinita. Immagini di macchine virtuali, file di database e core dump sono fra le voci più grandi su qualsiasi disco reale: la misura logica sbaglia di più proprio sulle voci che contano di più. La riga di comando usa logica come predefinita, ed entrambe dicono quale stanno mostrando.",
    sizeCapacityWhy:
      "La capacità è riportata come libero sul totale, mai come “% usato”. Su un filesystem il cui spazio è condiviso fra volumi — un container APFS, subvolumi btrfs, LVM thin — una cifra di spazio usato includerebbe i fratelli e contraddirebbe df sullo stesso mount.",
    sizeLinks:
      "Gli hardlink sono contati una sola volta per impostazione predefinita; la seconda copia appare nell'albero con un contributo di zero byte. I symlink non sono mai seguiti e contano per la propria dimensione.",

    refTitle: "Riferimento dei comandi",
    refOptionsTitle: "Opzioni che funzionano su quasi tutti i comandi",

    storageTitle: "Dove vengono conservate le cose",
    storagePlatform: "Piattaforma",
    storageDb: "Database delle istantanee",
    storageAnywhere: "Ovunque",
    storageOverride:
      "SPACETRACE_HOME lo sovrascrive; --db lo sovrascrive per un singolo comando",
    storageShared:
      "L'app desktop scrive nello stesso database, quindi un'istantanea salvata nell'app appare in spacetrace scans e viceversa.",

    limitsTitle: "Limiti noti",
    limitsLede:
      "Pubblicati invece di essere scoperti. Se ne incontri uno è debito noto, non una sorpresa.",
    limits: [
      "Su Windows la dimensione su disco è pari a quella logica e la deduplicazione degli hardlink è disattivata. Le cifre reali richiedono GetFileInformationByHandleEx e FileIdInfo.",
      "I cloni APFS non sono deduplicati, e su btrfs o ZFS reflink e compressione fanno sì che un attraversamento dell'albero non possa riportare l'uso reale. Nessun tree walker può; i numeri sono quelli dei file.",
      "Una scansione tiene l'intero albero in memoria. Il profilo di memoria oltre i dieci milioni di file non è stato misurato.",
      "L'agente non ha TLS né rate limiting. Usa un reverse proxy; il token è già obbligatorio.",
      "Lo scheduler non ha un database dei fusi orari — UTC più un offset fisso, quindi i cambi dell'ora legale restano da gestire a te.",
    ],
  },

  footer: {
    licence:
      "Lo scanner, l'archivio delle istantanee, lo strumento a riga di comando e l'agente sono Apache-2.0. L'app desktop e l'hub sono proprietari.",
    home: "Home",
    allReleases: "Tutte le release",
    builtWith: "Costruito con Astro. Nessun tracker, nessun cookie.",
  },
};
