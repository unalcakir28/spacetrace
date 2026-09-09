import type { Dictionary } from "./en";

export const fr: Dictionary = {
  locale: { label: "Langue", pick: "Choisir une langue" },

  nav: {
    desktop: "Application de bureau",
    cli: "Ligne de commande",
    hub: "Hub",
    download: "Télécharger",
    guide: "Guide",
    github: "GitHub",
    menu: "Menu",
    skip: "Aller au contenu",
  },

  common: {
    copy: "Copier",
    copied: "Copié",
    copyCommand: "Copier la commande",
    replay: "Rejouer",
    download: "Télécharger",
    free: "Libre",
    size: "Taille",
    file: "Fichier",
    platform: "Plateforme",
    notes: "Notes",
    version: "Version",
    latestBuild: "dernière build",
    stableRelease: "version stable",
    notARelease: "compilée depuis main, ce n'est pas une version publiée",
    checksums: "Sommes de contrôle",
    learnMore: "En savoir plus",
    sourceOnGitHub: "Code source sur GitHub",
    apacheCore: "Apache-2.0",
    proprietary: "Propriétaire",
    loading: "Chargement…",
  },

  home: {
    title: "spacetrace — voyez ce qui remplit votre disque, et ce qui a changé",
    description:
      "Une application de bureau qui cartographie ce qui remplit vos disques, conserve des instantanés et vous dit quel dossier a réellement grossi. Avec un outil en ligne de commande et un tableau de bord auto-hébergé pour les serveurs.",
    kicker: "macOS · Windows · Linux",
    headlineGiven: "Tous les analyseurs de disque cartographient ce qui est sur le disque.",
    headlineSecond: "Celui-ci montre aussi ce qui a changé.",
    lede: "Analysez un volume et parcourez-le comme une treemap vivante. Conservez le résultat comme instantané, revenez la semaine suivante et obtenez le seul dossier qui a vraiment grossi — pas la racine sous laquelle il se trouve.",
    ctaPrimary: "Télécharger l'application",
    ctaSecondary: "Ou utiliser la ligne de commande",
    noAccount: "Pas de compte, pas de télémétrie, rien n'est envoyé nulle part.",

    demoTitle: "C'est l'application. Faites un tour d'essai.",
    demoLede:
      "La vraie dessine des dizaines de milliers de tuiles sur un canvas. Celle-ci exécute le même agencement squarified sur un arbre plus petit, directement dans la page.",

    diffTitle: "Une analyse est une photographie. Deux sont une réponse.",
    diffLede:
      "Enregistrez une analyse comme instantané, puis comparez. Les dossiers qui ne font que transmettre une variation sont ignorés, et c'est le premier niveau où la croissance se répartit vraiment qui est signalé.",
    diffPoint:
      "Il nomme le dossier sur lequel vous pouvez agir, pas ses ancêtres.",

    partsTitle: "Un seul cœur, trois portes d'entrée",
    partsLede:
      "Le scanner, le stockage des instantanés, la comparaison et l'agencement de la treemap forment une seule bibliothèque Rust. Un instantané écrit par l'un est lisible par les deux autres.",

    trustTitle: "Des chiffres que vous pouvez vérifier",
    trustLede:
      "Un outil de disque qui affiche un chiffre irréconciliable avec votre propre shell a perdu la discussion. Ce sont des conditions de test dans le dépôt, pas des promesses.",

    ctaTitle: "Découvrez ce qui a grossi pendant que vous ne regardiez pas.",
    ctaLede:
      "L'application de bureau est téléchargeable gratuitement. Le scanner, l'outil en ligne de commande et l'agent serveur sont open source et le resteront.",
  },

  demo: {
    zoomHint: "Double-cliquez sur un dossier pour y entrer",
    backHint: "Remonter",
    rootLabel: "Dossier personnel",
    basisLabel: "Mesure",
    onDisk: "Sur le disque",
    logical: "Logique",
    basisHelp:
      "Sur le disque, ce sont les blocs réellement alloués. Logique, c'est la longueur que chaque fichier déclare. Elles diffèrent, et les deux sont justes.",
    sparseCallout:
      "Passez sur Logique et regardez l'image disque avaler la carte. Elle déclare un téraoctet et en occupe dix-neuf gigaoctets — voilà pourquoi l'application choisit sur le disque par défaut.",
    selected: "Sélection",
    share: "Part de cette analyse",
    kind: "Type",
    sparse: "creux",
    empty: "Rien de sélectionné",
    emptyHelp: "Cliquez sur une tuile pour l'inspecter.",
    entries: "entrées",
    tapHint: "Touchez deux fois un dossier pour y entrer",
  },

  categories: {
    image: "image",
    video: "vidéo",
    audio: "audio",
    document: "document",
    archive: "archive",
    code: "code",
    binary: "binaire",
    cache: "cache",
    other: "autre",
    directory: "dossier",
  },

  parts: {
    desktopName: "Application de bureau",
    desktopRole: "macOS · Windows · Linux",
    desktopPitch:
      "Une treemap navigable colorée par type de fichier, où la liste des dossiers, la carte et l'inspecteur s'accordent sur la mesure qu'ils affichent.",
    desktopPoints: [
      "Parcourez une analyse en direct, un instantané enregistré ou un serveur via HTTP",
      "Comparez deux instantanés et voyez le dossier qui a grossi",
      "Mettez des entrées à la corbeille sans perdre votre place",
      "La fenêtre reste utilisable pendant une analyse",
    ],
    desktopCta: "Ce que fait l'application",

    cliName: "Ligne de commande",
    cliRole: "un binaire statique · plus un agent serveur",
    cliPitch:
      "Le même cœur, sous forme de commande scriptable et de service qui analyse selon un calendrier et répond via HTTP.",
    cliPoints: [
      "Analyser, enregistrer, comparer, lister, exporter vers ncdu",
      "Pointez n'importe quelle commande en lecture seule vers un agent distant",
      "Tourne sur un NAS, dans un conteneur, sur de vieilles glibc",
      "L'agent lit. Il ne supprime jamais rien.",
    ],
    cliCta: "Commandes et installation",

    hubName: "Hub",
    hubRole: "auto-hébergé · un binaire, un fichier SQLite",
    hubPitch:
      "Les agents lui envoient leurs instantanés. Le hub conserve l'historique, calcule ce qui grossit et vous avertit avant qu'un disque ne soit plein.",
    hubPoints: [
      "Tableau de bord de la flotte trié par urgence, pas par nom d'hôte",
      "Taux de croissance et prévision « plein dans N jours »",
      "Règles de seuil livrées par webhook",
      "Ne touche jamais aux machines qu'il surveille",
    ],
    hubCta: "Lancer le hub",
  },

  trust: {
    duTitle: "Les totaux correspondent exactement à votre shell",
    duBody:
      "Logique ne compte que les octets des fichiers et correspond à du -sb. Sur le disque, ce sont les blocs réellement alloués, blocs de répertoires inclus, et cela correspond à du -s --block-size=1. Vérifié sur /usr avec 141k fichiers.",
    capacityTitle: "Libre sur total, jamais « % utilisé »",
    capacityBody:
      "La capacité est rapportée comme df la rapporte. Sur un conteneur APFS, un sous-volume btrfs ou un LVM thin, un chiffre d'espace utilisé inclurait les voisins et contredirait df sur le même point de montage — il n'est donc pas affiché.",
    forecastTitle: "La prévision est retenue plus souvent qu'affichée",
    forecastBody:
      "« Plein dans N jours » exige au moins trois instantanés couvrant au moins un jour, un ajustement linéaire de r² ≥ 0,5, une capacité réellement mesurée et une réponse à moins de dix ans. Une date fausse mais assurée est pire que pas de date.",
    snapshotTitle: "Un instantané est un simple fichier SQLite",
    snapshotBody:
      "Pas un format d'export, pas un blob propriétaire. Ce que l'agent conserve est ce qui circule sur le réseau et ce qui arrive sur votre portable : un instantané distant se parcourt avec le même code qu'un instantané local.",
    errorsTitle: "Les erreurs sont comptées, pas avalées",
    errorsBody:
      "Un chemin illisible est signalé et échantillonné ; l'analyse continue. Une analyse annulée ne renvoie aucun arbre, car un arbre partiel a l'air complet et annonce un total qui n'a jamais été vrai d'aucun disque.",
    deleteTitle: "L'agent n'a pas de suppression",
    deleteBody:
      "Un logiciel installé sur un serveur gagne la confiance en étant incapable de faire la chose effrayante. L'application de bureau peut mettre des entrées à la corbeille — seulement sur une analyse en direct de la machine devant vous, et seulement après avoir demandé.",
  },

  desktop: {
    title: "Application de bureau — spacetrace",
    description:
      "Une treemap navigable de ce qui remplit vos disques, colorée par type de fichier, avec instantanés, agents distants et comparaisons. macOS, Windows et Linux.",
    kicker: "Tauri et Rust · une fenêtre légère sur un scanner natif",
    headlineGiven: "Une carte de votre disque.",
    headlineSecond: "Puis une carte de ce qui a changé dessus.",
    lede: "Analysez un dossier et parcourez-le comme une treemap colorée par type de fichier. Ou ouvrez un instantané de la semaine dernière, ou un instantané posé sur un serveur, et mettez les deux côte à côte.",
    ctaPrimary: "Télécharger l'application",
    ctaSecondary: "Voir tous les fichiers",

    windowCaption:
      "Dessinée à partir de la feuille de style de l'application plutôt que capturée en image, elle reste donc fidèle aussi longtemps que la palette.",

    featuresTitle: "Ce que ça donne à l'usage",
    featuresLede:
      "L'application est une fenêtre sur le même cœur Rust que la ligne de commande. Son propre travail est de ne pas gêner.",

    f1Title: "La fenêtre ne vous est jamais retirée pendant le travail",
    f1Body:
      "Une analyse s'annonce dans un bandeau, pas derrière un rideau. Ce qui était ouvert reste ouvert et utilisable, et n'est remplacé que quand la nouvelle analyse se termine. Arrêtez-la, ou laissez-la échouer, et la précédente est toujours là.",
    f2Title: "Zoomez sans perdre votre place",
    f2Body:
      "Double-cliquez sur un dossier dans la carte, choisissez-en un dans le fil d'Ariane, ou utilisez le menu contextuel d'une ligne. Retour arrière remonte. Un clic dans la liste des dossiers ne déplace jamais la carte : parcourir ne peut pas vous faire perdre la vue.",
    f3Title: "La barre de progression admet quand elle devine",
    f3Body:
      "Parcourir un système de fichiers n'a pas de dénominateur. La seule estimation honnête est la dernière fois que cette application a analysé ce même dossier avec les mêmes options, et c'est celle qu'elle utilise — le bandeau dit d'où vient le nombre. Une première analyse affiche des compteurs et du mouvement, mais aucun pourcentage.",
    f4Title: "Supprimer ne recharge rien",
    f4Body:
      "Mettre des entrées à la corbeille modifie l'arbre ouvert sur place et corrige les totaux au-dessus : les dossiers dépliés restent dépliés, la carte ne bouge pas et le défilement reste où il était. Cent entrées, c'est le même appel qu'une seule, avec un vrai « 3 sur 11 ».",
    f5Title: "Mettre à la corbeille ne libère pas d'espace, et l'application le dit",
    f5Body:
      "Sur chaque plateforme prise en charge, la corbeille est sur le même système de fichiers : le dossier rétrécit, pas le disque. La confirmation le dit, l'avis ensuite le dit, et on peut cliquer sur l'espace libre pour le remesurer après avoir vidé la corbeille.",
    f6Title: "Les tailles disent toujours quelle mesure elles sont",
    f6Body:
      "Sur le disque par défaut, avec un commutateur étiqueté dans la barre d'outils plutôt qu'un réglage enterré. Le tri, le chiffre à côté et la couleur de la ligne viennent de la même mesure : « les plus gros d'abord » veut donc dire la même chose que le nombre imprimé sur la ligne.",

    sourcesTitle: "Trois sources, une fenêtre",
    sourcesLede:
      "L'application ne se soucie pas de la provenance d'un arbre, car un instantané est le même fichier SQLite où qu'il ait été écrit.",
    s1Title: "Cette machine",
    s1Body:
      "Une analyse en direct. La seule source où agir sur les fichiers est permis, parce que c'est la seule où les chemins signifient encore ce qu'ils disent.",
    s2Title: "Un instantané enregistré",
    s2Body:
      "Depuis la même base que celle écrite par la ligne de commande. Enregistrez l'analyse ouverte et elle apparaît dans spacetrace scans, et inversement.",
    s3Title: "Un agent distant",
    s3Body:
      "Pointez l'application vers l'URL et le jeton d'un agent et lisez ses instantanés via HTTP — sans jamais avoir à ouvrir une session sur le serveur.",
    s4Title: "Deux à la fois",
    s4Body:
      "Comparez deux instantanés quelconques de la même racine et voyez quel dossier a réellement grossi, avec la même règle du dossier coupable que la ligne de commande.",

    safetyTitle: "Ce qu'elle ne fera pas",
    safetyBody1:
      "L'application lit. La seule exception est Mettre à la corbeille, verrouillée de quatre façons : le back-end refuse si l'arbre ouvert n'est pas une analyse en direct de cette machine, il refuse la racine de l'analyse elle-même, il revérifie que chaque chemin existe encore, et la fenêtre demande d'abord — en listant ce qui va partir quand il y a plus d'un élément.",
    safetyBody2:
      "Rien n'est jamais supprimé directement. Cela va dans la corbeille du système, d'où vous pouvez le remettre.",
    safetyBody3:
      "Agir sur les fichiers est désactivé dès qu'un instantané est ouvert. Un instantané est la photographie du passé, ou d'une autre machine, et un chemin dedans peut ne plus signifier ce qu'il dit.",

    notYetTitle: "Pas encore fait",
    notYetLede: "Annoncé plutôt que découvert :",
    notYet: [
      "Le chemin rapide MFT sous Windows — les analyses y sont un parcours de répertoires classique",
      "Un écran d'accueil pour le Full Disk Access sur macOS",
      "Une vue chronologique de tout l'historique d'une cible, au lieu de deux instantanés à la fois",
      "Les performances de la treemap sur WebKitGTK, le moteur de rendu Linux, n'ont pas été optimisées",
    ],
  },

  cli: {
    title: "Ligne de commande — spacetrace",
    description:
      "L'outil en ligne de commande spacetrace et l'agent serveur : analyser, enregistrer, comparer et lire une machine distante via HTTP. Un binaire statique, Apache-2.0.",
    kicker: "Un binaire statique · aucun runtime à installer · Apache-2.0",
    headlineGiven: "Le même cœur, sans fenêtre.",
    headlineSecond: "Scriptable, et à sa place sur un serveur.",
    lede: "Tout ce que fait l'application, sous forme de commande à mettre dans une tâche cron — plus un agent qui analyse selon un calendrier et répond via HTTP, pour qu'une machine où vous ne vous connectez jamais puisse quand même vous dire ce qui est arrivé à son disque.",
    installTitle: "Installation",
    installNote:
      "Installe spacetrace et spacetrace-agent dans /usr/local/bin. Délibérément du sh POSIX sans surprise, parce qu'il doit aussi tourner sur le firmware d'un NAS dont le shell est busybox.",
    windowsNote:
      "Sous Windows, décompressez l'archive et placez spacetrace.exe dans votre PATH. Il n'y a pas d'installateur pour l'outil en ligne de commande.",

    toursTitle: "Les quatre commandes que vous utiliserez vraiment",
    tour1Title: "Regarder un dossier",
    tour1Body: "Le parcourt et affiche les totaux. Rien n'est enregistré, rien n'est modifié.",
    tour2Title: "Conserver le résultat",
    tour2Body: "Enregistre l'analyse comme instantané dans un simple fichier SQLite, avec une étiquette si vous voulez.",
    tour3Title: "Demander ce qui a changé",
    tour3Body: "Compare les deux derniers instantanés d'une racine, ou le plus récent avec le disque tel qu'il est maintenant.",
    tour4Title: "Lire une autre machine",
    tour4Body: "Toute commande en lecture seule accepte --remote. Mêmes sous-commandes, pointées ailleurs.",

    agentTitle: "L'agent",
    agentLede:
      "spacetrace-agent est le même code sous forme de service. Il analyse selon un calendrier les racines que vous configurez, conserve les instantanés et les sert via HTTP. Un binaire statique — et il ne fait que lire.",
    agentPoints: [
      "Calendriers cron à cinq champs par racine, avec rétention par nombre",
      "Jeton bearer obligatoire ; serve refuse de démarrer sans lui",
      "Écoute sur la boucle locale par défaut — exposer un inventaire doit être une décision délibérée",
      "/health n'exige aucun jeton, donc un healthcheck de conteneur fonctionne, et ne révèle rien d'autre",
      "Les clés de configuration inconnues sont refusées au démarrage plutôt qu'ignorées en silence",
    ],
    agentCta: "Installation complète de l'agent",

    dockerTitle: "Dans un conteneur",
    dockerNote:
      "Montez l'hôte en lecture seule et analysez-le. L'image contient amd64 et arm64.",

    openTitle: "Open source, et ça ne changera pas",
    openBody:
      "Le scanner, le stockage des instantanés, la comparaison et les deux binaires sont sous Apache-2.0. L'agent tourne sur vos serveurs, vous devez donc pouvoir le lire. L'application de bureau et le hub sont la partie commerciale.",
  },

  hub: {
    title: "Hub — spacetrace",
    description:
      "Le hub spacetrace auto-hébergé : les agents envoient des instantanés, le hub conserve l'historique, calcule ce qui grossit et vous avertit avant qu'un disque ne soit plein.",
    kicker: "Auto-hébergé · un binaire, un fichier SQLite · aucune télémétrie",
    headlineGiven: "Quarante machines, quarante disques.",
    headlineSecond: "Une page, le pire en premier.",
    lede: "Les agents analysent leurs propres machines et envoient les instantanés ici. Le hub conserve l'historique, calcule ce qui grossit et vous avertit avant qu'un disque ne soit plein. Il ne touche jamais aux machines qu'il surveille.",
    ctaBinaries: "Binaires",
    dashCaption:
      "Dessiné plutôt que capturé. Le vrai tableau de bord est du HTML rendu par le serveur — un outil auto-hébergé qui réclame un npm install avant de vous montrer une page est un outil moins bon.",
    withheldNote:
      "Une prévision vide est un refus de deviner, pas un nombre manquant. La page de la cible dit quelle condition n'était pas remplie.",

    setupTitle: "Mise en place",
    setupLede:
      "Deux identifiants, un fichier de configuration et une ligne ajoutée à chaque agent. Il n'y a rien d'autre à déployer.",
    step1Title: "Créez un jeton admin et une configuration",
    step1Body:
      "Enregistrez-la sous hub.toml, puis docker compose up -d — ou lancez le binaire directement et validez la configuration avant de servir.",
    step2Title: "Connectez-vous et créez un jeton d'agent",
    step2Body:
      "Ouvrez le hub, connectez-vous avec le contenu d'admin-token et créez un jeton sur la page Agents. Ou faites-le en ligne de commande.",
    step3Title: "Pointez chaque agent vers lui",
    step3Body:
      "Ajoutez cela au calendrier de l'agent et la flotte se remplit d'elle-même. Renvoyer est sans effet : un instantané déjà présent avec le même hôte, la même racine et la même heure de début est ignoré.",
    step4Title: "Mettez du TLS devant",
    step4Body:
      "Avant d'exposer le hub hors de votre réseau, terminez le TLS dans un reverse proxy. Le cookie de session est HttpOnly et SameSite=Strict ; le drapeau Secure vient du fait de le servir en HTTPS.",

    credsTitle: "Deux identifiants, volontairement",
    credsBody1:
      "Un jeton posé dans un fichier de configuration sur un NAS ne doit pas être la clé de l'inventaire de toute la flotte. Un jeton d'agent n'est donc pas un identifiant de tableau de bord — et l'inverse est vrai aussi.",
    credsBody2:
      "Les jetons d'agent sont stockés sous forme de hachages SHA-256 : une copie de la base n'est pas un jeu d'identifiants fonctionnels. Révoquer conserve la trace et rend le jeton inopérant immédiatement.",
    credsTested: "Les deux sens sont couverts par des tests.",
    credCol: "Identifiant",
    credCan: "Peut",
    credCannot: "Ne peut pas",
    agentToken: "Jeton d'agent",
    adminToken: "Jeton admin",
    agentCan: "Envoyer un instantané",
    agentCannot: "Lire le tableau de bord ou l'API",
    adminCan: "Tout lire, gérer les jetons et les règles",
    adminCannot: "Envoyer des instantanés",

    forecastTitle: "La prévision est la fonction la plus retenue",
    forecastLede:
      "C'est celle qui a le plus de chances d'être fausse, donc celle qui refuse le plus souvent de répondre. « Plein dans N jours » n'apparaît que si tout ceci est vrai.",
    fc1Title: "Au moins trois instantanés, couvrant au moins un jour",
    fc1Body:
      "Deux points font toujours une droite parfaite. Un jour est le plus court intervalle où un motif quotidien peut apparaître.",
    fc2Title: "Un ajustement linéaire de r² ≥ 0,5",
    fc2Body:
      "L'usage disque n'est souvent pas linéaire. Une rotation de journaux ou une restauration ponctuelle produisent volontiers une droite dont la pente ne veut rien dire.",
    fc3Title: "Une capacité réellement mesurée",
    fc3Body:
      "Sans la taille réelle du système de fichiers, il n'y a rien à remplir. Le hub ne la devine pas à partir du plus grand total qu'il a vu.",
    fc4Title: "Une réponse à moins de dix ans",
    fc4Body: "« Plein dans 4 000 jours » est de l'arithmétique, pas de l'information.",
    fc5Title: "La croissance et la prévision sont deux nombres différents",
    fc5Body:
      "La croissance est mesurée sur le dossier analysé. La prévision projette l'espace restant du système de fichiers à ce rythme. Les pages disent lequel est lequel plutôt que de les fondre en un seul chiffre rassurant.",

    alertsTitle: "Alertes",
    alertsLede:
      "Des règles de seuil, livrées par webhook. Trois types, chacun avec un délai de repos pour qu'un disque qui se remplit ne devienne pas un flot de messages identiques :",
    alerts: [
      "Espace libre sous un pourcentage",
      "Croissance au-dessus d'un rythme",
      "Une prévision qui tombe dans un horizon donné",
    ],
    alertsMissing:
      "L'envoi par e-mail n'est pas encore implémenté, et il y a un seul identifiant admin plutôt que des comptes par personne.",

    apiTitle: "API HTTP",
    noRollupTitle: "Pourquoi il n'y a pas de table d'agrégats",
    noRollupBody1:
      "Les instantanés sont conservés tels quels dans le même stockage que celui utilisé par la ligne de commande et l'agent. Le hub ne tient aucune table de résumé propre : rien ne peut donc dériver par rapport aux instantanés dont il découle.",
    noRollupBody2:
      "Ce qu'un agent envoie est le fichier même qu'il conserve localement : rien n'est converti d'un côté ni de l'autre, et la comparaison sur la page d'une cible est calculée par le même code que sur votre portable.",

    fleetHost: "Hôte",
    fleetRoot: "Racine",
    fleetFree: "Libre",
    fleetGrowth: "Croissance",
    fleetTrend: "14 derniers jours",
    fleetFills: "Plein dans",
    fleetDays: "jours",
    fleetTargets: "cibles",
    fleetReporting: "actives",
    fleetSilent: "silencieuse depuis 3 jours",
    fleetNoCapacity: "capacité non enregistrée",
    fleetTwoSnapshots: "2 instantanés",
  },

  download: {
    title: "Télécharger spacetrace",
    description:
      "Téléchargements et instructions d'installation pour l'application de bureau spacetrace, l'outil en ligne de commande et son agent, et le hub auto-hébergé. macOS, Windows, Linux, NAS et Docker.",
    headline: "Télécharger",
    lede: "Trois choses installables, toutes construites sur le même cœur. Prenez celle dont vous avez besoin ; un instantané écrit par l'une est lisible par les autres.",

    detectMac: "macOS",
    detectMacSub: "Universal — Apple silicon et Intel",
    detectWindows: "Windows",
    detectWindowsSub: "64 bits",
    detectLinux: "Linux",
    detectLinuxSub: "Binaires statiques — aucun runtime à installer",
    detectUnknown: "Choisissez une plateforme",
    detectUnknownSub:
      "Ce navigateur n'a pas dit sur quelle machine il se trouve. Les tableaux ci-dessous listent tous les fichiers.",
    goToFiles: "Aller aux listes de fichiers",
    alsoCli: "Outil en ligne de commande et agent",
    cliAlongside:
      "L'application et l'outil en ligne de commande partagent une seule base d'instantanés : ils peuvent donc être utilisés ensemble.",

    continuousTitle: "Ce que « continuous » signifie",
    continuousBody:
      "Jusqu'à la première version étiquetée, chaque téléchargement ici est la build la plus récente de main : elle a passé la CI et rien d'autre. Dès qu'une version stable existera, cette page basculera dessus automatiquement.",

    desktopSection: "Application de bureau",
    desktopSectionLede:
      "La fenêtre à treemap. Téléchargement gratuit tant qu'elle est en phase 3.",
    desktopCaption:
      "La build macOS est universal : il n'y a qu'un fichier pour Apple silicon et Intel.",

    cliSection: "Ligne de commande et agent",
    cliSectionLede:
      "Une seule archive contient les deux binaires : spacetrace pour la ligne de commande et spacetrace-agent pour le service qui analyse selon un calendrier et répond via HTTP.",
    cliCaption:
      "Les builds Linux sont en musl statique : elles tournent sur de vieilles glibc et dans un conteneur scratch. Chaque archive contient aussi le README et la licence.",

    hubSection: "Hub",
    hubSectionLede:
      "La vue de la flotte. Un binaire, un fichier SQLite, aucun serveur de base de données et aucune build frontend.",
    hubCaption:
      "L'image du conteneur contient les deux architectures : docker pull prend la bonne sans suffixe de tag.",

    unsignedTitle: "Rien ici n'est signé",
    unsignedBody:
      "Les certificats de signature coûtent de l'argent chaque année et ne peuvent pas vivre dans un dépôt public : macOS et Windows vous arrêteront tous les deux la première fois. Cela mérite de la méfiance, et c'est pourquoi une somme de contrôle est publiée à côté de chaque fichier et la build est un workflow que vous pouvez lire.",

    installMacTitle: "macOS : « impossible d'ouvrir spacetrace »",
    installMacBody:
      "Ouvrez le .dmg, glissez l'application dans Applications, puis faites un clic droit et choisissez Ouvrir — c'est cela qui donne l'autorisation unique que le double-clic ne propose pas. Ou retirez directement l'attribut de quarantaine :",
    installMacFda:
      "Analyser en dehors de votre dossier personnel exige le Full Disk Access pour l'application, dans Réglages Système → Confidentialité et sécurité.",
    installWinTitle: "Windows : SmartScreen",
    installWinBody:
      "« Windows a protégé votre ordinateur » → Informations complémentaires → Exécuter quand même. L'installateur est par utilisateur et n'exige aucun droit d'administrateur.",
    installLinuxTitle: "Linux",
    installLinuxBody:
      "L'AppImage n'a besoin d'aucune installation : rendez-la exécutable et lancez-la. Sous Wayland, si la fenêtre reste blanche, lancez-la avec WEBKIT_DISABLE_DMABUF_RENDERER=1 — c'est un problème de WebKitGTK, pas de l'application.",

    verifyTitle: "Vérifiez ce que vous avez téléchargé",
    verifyBody:
      "Chaque version porte un fichier SHA256SUMS couvrant ses propres fichiers. Vérifiez avant d'installer, d'autant qu'aucun binaire n'est signé.",
    verifyMacNote: "macOS n'a pas sha256sum ; utilisez shasum -a 256 -c.",

    buildTitle: "Ou compilez-le vous-même",
    buildBody:
      "Le cœur, l'outil en ligne de commande et l'agent sont sous Apache-2.0 et n'exigent que Rust 1.85 ou plus récent.",
    buildNote:
      "L'application de bureau et le hub ne sont pas open source et ne peuvent donc pas être compilés depuis les sources — c'est la contrepartie du cœur qui reste sous licence permissive.",

    platMacArm: "macOS, Apple silicon",
    platMacIntel: "macOS, Intel",
    platMacAny: "macOS 11+",
    platWin: "Windows 10 / 11",
    platLinuxX64: "Linux, x86_64",
    platLinuxArm: "Linux, aarch64",
    platDebian: "Debian, Ubuntu",
    platFedora: "Fedora, RHEL",
    platAnyLinux: "N'importe quel Linux",
    noteUniversal: "Binaire universal",
    noteNsis: "Installateur NSIS, par utilisateur",
    noteDeb: "Nécessite libwebkit2gtk-4.1",
    noteRpm: "Nécessite webkit2gtk4.1",
    noteAppImage: "Rendez-la exécutable et lancez-la ; rien n'est installé",
    noteStatic: "Statique ; serveurs et conteneurs",
    noteNas: "Raspberry Pi, serveurs ARM, la plupart des NAS",
    noteM1: "M1 et plus récent",
    noteMac11: "macOS 11+",
    noteWinAlloc:
      "Ici la taille sur le disque égale la taille logique ; voir les limites connues",
    noteAllArchives: "Un seul fichier couvrant toutes les archives ci-dessus",
    noteHubServer: "Statique ; le choix habituel pour un serveur",
    noteHubTry: "Pour l'essayer d'abord sur votre machine",
  },

  guide: {
    title: "Guide — spacetrace",
    description:
      "Comment utiliser spacetrace : analyser, conserver des instantanés, en comparer deux, installer l'agent sur un serveur, lire une machine distante, et ce que signifie chaque mesure de taille.",
    headline: "Guide",
    lede: "Tout ce que fait l'outil en ligne de commande, dans l'ordre où vous en aurez probablement besoin. --help sur n'importe quelle commande affiche la même chose sans navigateur.",
    onThisPage: "Sur cette page",

    startTitle: "Première analyse",
    startBody:
      "Une analyse parcourt un dossier et affiche ce qu'elle a trouvé. Rien n'est enregistré, rien n'est modifié.",
    startUnreadable:
      "Les chemins illisibles sont comptés et échantillonnés au lieu d'arrêter le parcours. Sur macOS, sortir de votre dossier personnel exige le Full Disk Access pour votre terminal ; sans lui, l'analyse se termine quand même et vous dit ce qu'elle a manqué.",
    startNarrowTitle: "La rendre plus rapide et plus étroite",
    startExclude:
      "--exclude prend un nom de répertoire, pas un chemin, et se répète. On ne descend jamais dans les dossiers exclus : exclure node_modules sur une machine de développement fait généralement la différence entre deux secondes et trente.",

    snapTitle: "Instantanés et comparaison",
    snapBody:
      "C'est la partie que les autres analyseurs n'ont pas. Enregistrez une analyse, enregistrez-en une autre plus tard, et demandez ce qui a changé entre les deux.",
    snapThen: "Puis comparez. Sans argument, il prend les deux derniers.",
    culpritTitle: "Pourquoi il nomme le dossier le plus profond",
    culpritBody:
      "Un dossier dont la croissance vient entièrement d'un seul enfant ne vous apprend rien, il est donc ignoré. Le rapport nomme le premier niveau où la variation se répartit vraiment — le coupable, pas ses ancêtres.",
    snapSinceTitle: "Contre le disque tel qu'il est maintenant",
    snapSinceBody:
      "Vous n'avez pas besoin de deux instantanés. Comparez le plus récent avec une analyse en direct :",
    snapFilterTitle: "Choisir ce qui apparaît",
    snapPruneTitle: "Empêcher la base de grossir indéfiniment",

    browseTitle: "Parcourir une analyse",
    browseBody:
      "ls liste les dossiers par taille, depuis une analyse en direct ou depuis un instantané enregistré.",
    browseExport:
      "Pour passer un instantané à autre chose, export écrit le format de ncdu :",
    browseJson:
      "Chaque commande accepte aussi --json, qui est la manière prise en charge de scripter par-dessus. Les colonnes lisibles peuvent changer ; la forme du JSON, non.",

    agentTitle: "L'agent sur un serveur",
    agentBody:
      "spacetrace-agent est le même code sous forme de service : il analyse selon un calendrier les racines que vous configurez, conserve les instantanés et répond via HTTP. C'est un binaire statique, et il ne fait que lire — il n'existe en lui aucun chemin de code qui supprime quoi que ce soit en dehors de sa propre base d'instantanés.",
    agentStep1: "Écrivez une configuration et un jeton",
    agentStep1Body:
      "serve refuse de démarrer sans jeton. Un agent sans authentification livre l'inventaire complet de son système de fichiers à quiconque atteint le port.",
    agentStep2: "Dites quoi analyser et quand",
    agentStep2Body:
      "Cinq champs cron, avec *, a-b, */n et des listes. Pas de secondes, pas de @daily. Les clés de configuration inconnues sont refusées au démarrage, car une faute de frappe qui ne fait rien en silence sur une machine que personne ne regarde est pire qu'un refus de démarrer.",
    agentStep3: "Vérifiez avant de le lancer",
    agentStep3Body:
      "Un calendrier qui ne peut jamais se déclencher affiche never au lieu d'échouer en silence.",
    agentStep4: "Lancez-le",
    agentStep4Body:
      "L'unité systemd fournie exécute l'agent sous son propre utilisateur non privilégié avec ProtectSystem=strict, à Nice=10 et en priorité d'E/S idle. Une analyse ne doit jamais gêner ce à quoi la machine sert vraiment.",
    agentDocker: "Sous Docker, montez l'hôte en lecture seule et analysez-le :",
    agentSecTitle: "Avant de l'exposer",
    agentSec: [
      "Il écoute sur la boucle locale par défaut. Publier l'inventaire d'un système de fichiers sur un réseau doit être une décision délibérée.",
      "Il n'y a pas de TLS dans l'agent. Mettez un reverse proxy devant.",
      "Le jeton est comparé sans sortie anticipée : un mauvais jeton met le même temps à être rejeté, quelle qu'en soit la part correcte.",
      "/health n'exige aucun jeton, donc un healthcheck de conteneur fonctionne, et ne renvoie que l'état et la version — aucun nom d'hôte, aucune racine.",
      "Les analyses à la demande sont désactivées. Activées, quiconque détient le jeton peut énumérer tout répertoire lisible par l'utilisateur de l'agent.",
    ],

    remoteTitle: "Lire une autre machine",
    remoteBody:
      "Toute commande en lecture seule accepte --remote. Rien de nouveau à apprendre : les mêmes sous-commandes, pointées ailleurs.",
    remoteSaveBody:
      "Enregistrez un serveur distant pour ne plus taper l'URL et le jeton — dans ~/.config/spacetrace/remotes.toml :",
    remoteNotDifferentTitle: "Un instantané distant n'est pas une chose différente",
    remoteNotDifferentBody:
      "Ce qui arrive par le réseau est le même fichier SQLite autonome que l'agent conserve : le lister, le parcourir et le comparer exécute exactement le même code qu'un instantané local. Vous pouvez même sauter l'outil et obtenir quand même un fichier ouvrable :",
    remoteRefuse:
      "scan, prune et rm refusent de tourner avec --remote : ils agissent sur l'état local, et l'agent ne supprime rien.",

    sizeTitle: "Quelle taille, et pourquoi",
    sizeBody: "Chaque entrée porte deux nombres, et aucun n'est une estimation de l'autre.",
    sizeMeasure: "Mesure",
    sizeWhat: "Ce que c'est",
    sizeMatches: "Correspond à",
    sizeLogical: "logique",
    sizeLogicalWhat: "La longueur que chaque fichier déclare",
    sizeOnDisk: "sur le disque",
    sizeOnDiskWhat: "Blocs réellement alloués, blocs de répertoires inclus",
    sizeCapacity: "capacité",
    sizeCapacityWhat: "Espace libre sur le total, sur le système de fichiers analysé",
    sizeCapacityMatches: "la colonne Avail de df, exactement",
    sizeDiverge:
      "Elles divergent dans les deux sens et les deux sont justes. Un fichier d'un octet alloue un bloc entier : sur le disque, il est plus gros que sa longueur. Un fichier creux déclare une longueur qu'il n'a jamais allouée : une image disque peut déclarer un téraoctet et n'en occuper que dix-neuf gigaoctets.",
    sizeWhyOnDisk:
      "Les fichiers creux sont la raison pour laquelle l'application choisit sur le disque par défaut. Les images de machines virtuelles, les fichiers de base de données et les core dumps figurent parmi les plus grosses entrées de tout disque réel : la mesure logique se trompe donc le plus sur les entrées qui comptent le plus. La ligne de commande utilise logique par défaut, et les deux disent laquelle elles affichent.",
    sizeCapacityWhy:
      "La capacité est rapportée comme libre sur total, jamais comme « % utilisé ». Sur un système de fichiers dont l'espace est partagé entre volumes — conteneur APFS, sous-volumes btrfs, LVM thin — un chiffre d'espace utilisé inclurait les voisins et contredirait df sur le même point de montage.",
    sizeLinks:
      "Les liens physiques sont comptés une seule fois par défaut ; la seconde copie apparaît dans l'arbre avec une contribution de zéro octet. Les liens symboliques ne sont jamais suivis et comptent pour leur propre taille.",

    refTitle: "Référence des commandes",
    refOptionsTitle: "Options valables sur la plupart des commandes",

    storageTitle: "Où les choses sont conservées",
    storagePlatform: "Plateforme",
    storageDb: "Base des instantanés",
    storageAnywhere: "N'importe où",
    storageOverride:
      "SPACETRACE_HOME la remplace ; --db la remplace pour une seule commande",
    storageShared:
      "L'application de bureau écrit dans la même base : un instantané enregistré dans l'application apparaît dans spacetrace scans, et inversement.",

    limitsTitle: "Limites connues",
    limitsLede:
      "Publiées plutôt que découvertes. Si vous en rencontrez une, c'est une dette connue, pas une surprise.",
    limits: [
      "Sous Windows, la taille sur le disque égale la taille logique et la déduplication des liens physiques est désactivée. Les vrais chiffres exigent GetFileInformationByHandleEx et FileIdInfo.",
      "Les clones APFS ne sont pas dédupliqués, et sur btrfs ou ZFS les reflinks et la compression font qu'un parcours d'arbre ne peut pas rapporter l'usage réel. Aucun parcours d'arbre ne peut ; les nombres sont ceux des fichiers.",
      "Une analyse garde tout l'arbre en mémoire. Le profil mémoire au-delà de dix millions de fichiers n'a pas été mesuré.",
      "L'agent n'a ni TLS ni limitation de débit. Utilisez un reverse proxy ; le jeton est déjà obligatoire.",
      "Le planificateur n'a pas de base de fuseaux horaires — UTC plus un décalage fixe : les changements d'heure restent à votre charge.",
    ],
  },

  footer: {
    licence:
      "Le scanner, le stockage des instantanés, l'outil en ligne de commande et l'agent sont sous Apache-2.0. L'application de bureau et le hub sont propriétaires.",
    home: "Accueil",
    allReleases: "Toutes les versions",
    builtWith: "Construit avec Astro. Aucun traceur, aucun cookie.",
  },
};
