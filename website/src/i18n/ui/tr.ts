import type { Dictionary } from "./en";

export const tr: Dictionary = {
  locale: {
    label: "Dil",
    pick: "Bir dil seçin",
    autoNotice: "Tarayıcınızın diline göre Türkçe gösteriliyor.",
    dismiss: "Kapat",
  },

  nav: {
    desktop: "Masaüstü uygulaması",
    cli: "Komut satırı",
    hub: "Hub",
    download: "İndir",
    guide: "Kılavuz",
    github: "GitHub",
    menu: "Menü",
    skip: "İçeriğe geç",
  },

  common: {
    copy: "Kopyala",
    copied: "Kopyalandı",
    copyCommand: "Komutu kopyala",
    replay: "Yeniden oynat",
    download: "İndir",
    free: "Boş",
    size: "Boyut",
    file: "Dosya",
    platform: "Platform",
    notes: "Notlar",
    version: "Sürüm",
    latestBuild: "son derleme",
    stableRelease: "kararlı sürüm",
    notARelease: "main'den derlendi, yayınlanmış bir sürüm değil",
    checksums: "Sağlama toplamları",
    learnMore: "Ayrıntılar",
    sourceOnGitHub: "GitHub'da kaynak kodu",
    apacheCore: "Apache-2.0",
    proprietary: "Ticari",
    loading: "Yükleniyor…",
  },

  home: {
    title: "spacetrace — diskini neyin doldurduğunu ve neyin değiştiğini gör",
    description:
      "Diskleri neyin doldurduğunu haritalayan, anlık görüntü tutan ve hangi klasörün gerçekten büyüdüğünü söyleyen bir masaüstü uygulaması. Yanında komut satırı aracı ve sunucular için kendi sunucunuzda çalışan bir filo panosu.",
    kicker: "macOS · Windows · Linux",
    headlineGiven: "Her disk analizcisi diskte ne olduğunu haritalar.",
    headlineSecond: "Bu, neyin değiştiğini de gösterir.",
    lede: "Bir diski tara ve canlı bir treemap olarak gez. Sonucu anlık görüntü olarak sakla, haftaya geri dön ve gerçekten büyüyen tek klasörü al — altında durduğu kök dizini değil.",
    ctaPrimary: "Uygulamayı indir",
    ctaSecondary: "Ya da komut satırını kullan",
    noAccount: "Hesap yok, telemetri yok, hiçbir şey hiçbir yere yüklenmiyor.",

    demoTitle: "Uygulama bu. Bir tur at.",
    demoLede:
      "Gerçeği bir canvas üzerinde on binlerce karo çiziyor. Buradaki aynı squarified yerleşimi daha küçük bir ağaçta, tam bu sayfada çalıştırıyor.",

    diffTitle: "Bir tarama fotoğraftır. İki tarama cevaptır.",
    diffLede:
      "Bir taramayı anlık görüntü olarak sakla, sonra karşılaştır. Değişimi yalnızca aktaran klasörler atlanıyor ve büyümenin gerçekten dağıldığı ilk seviye bildiriliyor.",
    diffPoint:
      "Üzerinde işlem yapabileceğin klasörü söylüyor, onun atalarını değil.",

    partsTitle: "Tek çekirdek, üç giriş yolu",
    partsLede:
      "Tarayıcı, anlık görüntü deposu, karşılaştırma ve treemap yerleşimi tek bir Rust kütüphanesi. Bunlardan herhangi biriyle yazılan bir anlık görüntüyü diğer ikisi okuyabiliyor.",

    trustTitle: "Doğrulayabileceğin sayılar",
    trustLede:
      "Kendi kabuğunla doğrulayamadığın bir rakam basan bir disk aracı tartışmayı kaybetmiştir. Bunlar depoda birer test koşulu, verilmiş sözler değil.",

    ctaTitle: "Sen bakmazken neyin büyüdüğünü öğren.",
    ctaLede:
      "Masaüstü uygulaması ücretsiz indirilebiliyor. Tarayıcı, komut satırı aracı ve sunucu ajanı açık kaynak ve öyle kalacak.",
  },

  demo: {
    zoomHint: "Yakınlaşmak için bir klasöre çift tıkla",
    backHint: "Yukarı çık",
    rootLabel: "Ana dizin",
    basisLabel: "Ölçü",
    onDisk: "Diskte",
    logical: "Mantıksal",
    basisHelp:
      "Diskte, gerçekten tahsis edilmiş bloklar. Mantıksal, her dosyanın iddia ettiği uzunluk. Birbirinden farklı ve ikisi de doğru.",
    sparseCallout:
      "Mantıksal'a geç ve disk imajının haritayı yuttuğunu izle. Bir terabayt iddia ediyor, on dokuz gigabayt tutuyor — uygulamanın diskte ölçüsünü varsayılan yapma sebebi bu.",
    selected: "Seçili",
    share: "Bu taramadaki payı",
    kind: "Tür",
    sparse: "seyrek",
    empty: "Hiçbir şey seçilmedi",
    emptyHelp: "İncelemek için bir karoya tıkla.",
    entries: "girdi",
    tapHint: "Yakınlaşmak için klasöre iki kez dokun",
  },

  categories: {
    image: "görüntü",
    video: "video",
    audio: "ses",
    document: "belge",
    archive: "arşiv",
    code: "kod",
    binary: "ikili",
    cache: "önbellek",
    other: "diğer",
    directory: "klasör",
  },

  parts: {
    desktopName: "Masaüstü uygulaması",
    desktopRole: "macOS · Windows · Linux",
    desktopPitch:
      "Dosya türüne göre renklenen, yakınlaşabilir bir treemap; klasör listesi, harita ve inceleme panelinin hangi ölçüyü gösterdiği konusunda hemfikir olduğu bir pencere.",
    desktopPoints: [
      "Canlı bir taramayı, saklanmış bir anlık görüntüyü ya da HTTP üzerinden bir sunucuyu gez",
      "İki anlık görüntüyü karşılaştır ve büyüyen klasörü gör",
      "Girdileri, yerini kaybetmeden Çöp Kutusu'na taşı",
      "Tarama sürerken pencere kullanılabilir kalıyor",
    ],
    desktopCta: "Uygulama ne yapıyor",

    cliName: "Komut satırı",
    cliRole: "tek statik ikili · artı bir sunucu ajanı",
    cliPitch:
      "Aynı çekirdek; hem betikleyebileceğin bir komut, hem de zamanlanmış tarama yapıp HTTP üzerinden cevap veren bir servis olarak.",
    cliPoints: [
      "Tara, anlık görüntü al, karşılaştır, listele, ncdu'ya aktar",
      "Salt okunur her komutu uzak bir ajana yönlendir",
      "NAS'ta, konteynerde, yaşlı glibc üzerinde çalışıyor",
      "Ajan okur. Hiçbir şeyi silmiyor.",
    ],
    cliCta: "Komutlar ve kurulum",

    hubName: "Hub",
    hubRole: "kendi sunucunda · tek ikili, tek SQLite dosyası",
    hubPitch:
      "Ajanlar anlık görüntülerini buraya gönderiyor. Hub geçmişi tutuyor, neyin büyüdüğünü hesaplıyor ve bir disk dolmadan önce sana söylüyor.",
    hubPoints: [
      "Aciliyete göre sıralanan filo panosu, sunucu adına göre değil",
      "Büyüme hızı ve “N gün içinde dolar” tahmini",
      "Webhook ile iletilen eşik kuralları",
      "İzlediği makinelere hiç dokunmuyor",
    ],
    hubCta: "Hub'ı çalıştır",
  },

  trust: {
    duTitle: "Toplamlar kabuğunla birebir aynı",
    duBody:
      "Mantıksal yalnızca dosya baytları ve du -sb ile birebir. Diskte, gerçekten tahsis edilen bloklar — dizin blokları dâhil — ve du -s --block-size=1 ile birebir. 141 bin dosyalı /usr üzerinde doğrulandı.",
    capacityTitle: "Toplamın boşu, asla “% dolu” değil",
    capacityBody:
      "Kapasite df'in raporladığı gibi raporlanıyor. Bir APFS konteynerinde, btrfs alt biriminde ya da ince LVM'de “kullanılan” rakamı kardeşleri de içerir ve aynı bağlama noktası için df ile çelişir — bu yüzden basılmıyor.",
    forecastTitle: "Tahmin, gösterildiğinden daha sık gizleniyor",
    forecastBody:
      "“N gün içinde dolar” için en az bir günü kapsayan üç anlık görüntü, r² ≥ 0,5 doğrusal uyum, gerçekten ölçülmüş bir kapasite ve on yıl içinde bir cevap gerekiyor. Kendinden emin ama yanlış bir tarih, tarih olmamasından kötüdür.",
    snapshotTitle: "Anlık görüntü düpedüz bir SQLite dosyası",
    snapshotBody:
      "Bir dışa aktarma biçimi değil, kapalı bir blob değil. Ajanın sakladığı şey telde giden şeyle ve dizüstüne inen şeyle aynı; yani uzak bir anlık görüntü yerel biriyle birebir aynı kod tarafından gezilir.",
    errorsTitle: "Hatalar sayılıyor, yutulmuyor",
    errorsBody:
      "Okunamayan yol bildiriliyor ve örnekleniyor; tarama devam ediyor. İptal edilen tarama hiç ağaç döndürmüyor, çünkü kısmi bir ağaç tam görünür ve hiçbir diskte hiç doğru olmamış bir toplam bildirir.",
    deleteTitle: "Ajanda silme yok",
    deleteBody:
      "Sunucuya kurduğun yazılım, korkutucu şeyi yapamayarak güven kazanır. Masaüstü uygulaması girdileri Çöp Kutusu'na taşıyabiliyor — yalnızca başında oturduğun makinenin canlı taramasında ve yalnızca sorduktan sonra.",
  },

  desktop: {
    title: "Masaüstü uygulaması — spacetrace",
    description:
      "Diskleri neyin doldurduğunu dosya türüne göre renklenmiş, yakınlaşabilir bir treemap olarak gösteren uygulama; anlık görüntüler, uzak ajanlar ve karşılaştırma ile. macOS, Windows ve Linux.",
    kicker: "Tauri ve Rust · doğal bir tarayıcının üstünde küçük bir pencere",
    headlineGiven: "Diskinin haritası.",
    headlineSecond: "Sonra üzerinde neyin değiştiğinin haritası.",
    lede: "Bir klasörü tara ve dosya türüne göre renklenmiş bir treemap olarak gez. Ya da geçen haftadan bir anlık görüntüyü, ya da bir sunucuda duran birini aç ve ikisini yan yana koy.",
    ctaPrimary: "Uygulamayı indir",
    ctaSecondary: "Bütün dosyaları gör",

    windowCaption:
      "Ekran görüntüsü değil, uygulamanın kendi stil dosyasından çizildi; yani palet değişmedikçe doğru kalıyor.",

    featuresTitle: "Kullanırken nasıl bir şey",
    featuresLede:
      "Uygulama, komut satırının kullandığı aynı Rust çekirdeğine açılan bir pencere. Kendi işi yoldan çekilmek.",

    f1Title: "Çalışırken pencere elinden alınmıyor",
    f1Body:
      "Tarama kendini bir şeritte bildiriyor, perde arkasında değil. Açık olan ne varsa açık ve kullanılabilir kalıyor, ancak yeni tarama bittiğinde değiştiriliyor. Durdur ya da başarısız olsun — önceki tarama hâlâ orada.",
    f2Title: "Yerini kaybetmeden yakınlaş",
    f2Body:
      "Haritada bir klasöre çift tıkla, kırıntı yolundan birini seç ya da satırın bağlam menüsünü kullan. Backspace yukarı çıkarıyor. Klasör listesindeki bir tıklama haritayı hiç oynatmıyor, yani gezinmek baktığın görünümü kaybettiremez.",
    f3Title: "İlerleme çubuğu tahmin ettiğini itiraf ediyor",
    f3Body:
      "Bir dosya sistemi yürüyüşünün paydası yok. Tek dürüst tahmin, bu uygulamanın aynı klasörü aynı seçeneklerle en son taradığı zaman — kullandığı da bu, ve şerit sayının nereden geldiğini yazıyor. İlk taramada sayaçlar ve hareket var, yüzde yok.",
    f4Title: "Silmek hiçbir şeyi yeniden yüklemiyor",
    f4Body:
      "Girdileri Çöp Kutusu'na taşımak açık ağacı yerinde düzenliyor ve üstündeki toplamları düzeltiyor; yani açtığın klasörler açık kalıyor, harita yerinde duruyor ve kaydırma konumu oynamıyor. Yüz girdi, bir girdiyle aynı çağrı — ve gerçek bir “11'in 3'ü” ile.",
    f5Title: "Çöp Kutusu'na taşımak yer açmıyor, uygulama da bunu söylüyor",
    f5Body:
      "Desteklenen her platformda Çöp Kutusu aynı dosya sisteminde, yani klasör küçülüyor ama disk küçülmüyor. Onay bunu söylüyor, sonraki bildirim de söylüyor; boş alan göstergesine tıklayıp kutuyu boşalttıktan sonra yeniden ölçebiliyorsun.",
    f6Title: "Boyutlar hangi ölçü olduğunu her zaman söylüyor",
    f6Body:
      "Varsayılan diskte, ve araç çubuğunda etiketli bir anahtarla — gömülü bir ayarla değil. Sıralama, yanındaki rakam ve satırın rengi aynı ölçüden geliyor; yani “en büyük önce” yanındaki sayıyla aynı şeyi söylüyor.",

    sourcesTitle: "Üç kaynak, tek pencere",
    sourcesLede:
      "Uygulama bir ağacın nereden geldiğine bakmıyor, çünkü anlık görüntü nerede yazılmışsa aynı SQLite dosyası.",
    s1Title: "Bu makine",
    s1Body:
      "Canlı tarama. Dosyalar üzerinde işlem yapmanın izinli olduğu tek kaynak, çünkü yolların hâlâ söylediği şeyi ifade ettiği tek kaynak.",
    s2Title: "Saklanmış bir anlık görüntü",
    s2Body:
      "Komut satırının yazdığı aynı veritabanından. Açık taramayı kaydet, spacetrace scans içinde görünüyor — tersi de geçerli.",
    s3Title: "Uzak bir ajan",
    s3Body:
      "Uygulamayı bir ajanın adresine ve tokenına yönlendir, anlık görüntülerini HTTP üzerinden oku — sunucuya hiç bağlanmak zorunda kalmadan.",
    s4Title: "Aynı anda ikisi",
    s4Body:
      "Aynı kökün herhangi iki anlık görüntüsünü karşılaştır ve hangi klasörün gerçekten büyüdüğünü gör; komut satırının kullandığı aynı suçlu-klasör kuralıyla.",

    safetyTitle: "Yapmayacağı şeyler",
    safetyBody1:
      "Uygulama okur. Tek istisna Çöp Kutusu'na Taşı, ve dört yerden kapılı: arka uç, açık ağaç bu makinenin canlı taraması değilse reddediyor, tarama kökünün kendisini reddediyor, her yolun hâlâ var olduğunu yeniden kontrol ediyor ve pencere önce soruyor — birden fazlaysa neyin gideceğini listeleyerek.",
    safetyBody2:
      "Hiçbir şey doğrudan silinmiyor. Sistemin Çöp Kutusu'na gidiyor, oradan geri koyabiliyorsun.",
    safetyBody3:
      "Bir anlık görüntü açıkken dosyalar üzerinde işlem devre dışı. Anlık görüntü geçmişin ya da başka bir makinenin fotoğrafı ve içindeki bir yol artık söylediği şeyi ifade etmiyor olabilir.",

    notYetTitle: "Henüz yapılmadı",
    notYetLede: "Keşfedilmesi beklenmeden yazıldı:",
    notYet: [
      "Windows MFT hızlı yolu — oradaki taramalar normal bir dizin yürüyüşü",
      "macOS Full Disk Access karşılama ekranı",
      "Bir hedefin bütün geçmişini gösteren zaman çizelgesi görünümü; şimdilik aynı anda iki anlık görüntü",
      "Linux tarafındaki oluşturucu WebKitGTK'da treemap performansı ayarlanmadı",
    ],
  },

  cli: {
    title: "Komut satırı — spacetrace",
    description:
      "spacetrace komut satırı aracı ve sunucu ajanı: tara, anlık görüntü al, karşılaştır ve HTTP üzerinden uzak bir makineyi oku. Tek statik ikili, Apache-2.0.",
    kicker: "Tek statik ikili · kurulacak çalışma zamanı yok · Apache-2.0",
    headlineGiven: "Aynı çekirdek, pencere olmadan.",
    headlineSecond: "Betiklenebilir, ve sunucuda evinde.",
    lede: "Uygulamanın yaptığı her şey, bir cron işine koyabileceğin bir komut olarak — artı zamanlanmış tarama yapıp HTTP üzerinden cevap veren bir ajan; böylece hiç bağlanmadığın bir makine bile diskine ne olduğunu söyleyebiliyor.",
    installTitle: "Kurulum",
    installNote:
      "spacetrace ve spacetrace-agent'ı /usr/local/bin içine kuruyor. Bilinçli olarak sıkıcı POSIX sh, çünkü kabuğu busybox olan NAS yazılımlarında da çalışması gerekiyor.",
    windowsNote:
      "Windows'ta arşivi aç ve spacetrace.exe'yi PATH üzerinde bir yere koy. Komut satırı aracı için kurulum sihirbazı yok.",

    toursTitle: "Gerçekten kullanacağın dört komut",
    tour1Title: "Bir klasöre bak",
    tour1Body: "Yürüyor ve toplamları basıyor. Hiçbir şey saklanmıyor, hiçbir şey değişmiyor.",
    tour2Title: "Sonucu sakla",
    tour2Body: "Taramayı düz bir SQLite dosyasında anlık görüntü olarak saklıyor, istersen etiketle.",
    tour3Title: "Neyin değiştiğini sor",
    tour3Body: "Bir kökün son iki anlık görüntüsünü, ya da en yenisini diskin şu anki hâliyle karşılaştırıyor.",
    tour4Title: "Başka bir makineyi oku",
    tour4Body: "Salt okunur her komut --remote alıyor. Aynı alt komutlar, başka yere yönlenmiş.",

    agentTitle: "Ajan",
    agentLede:
      "spacetrace-agent aynı kodun servis hâli. Yapılandırdığın kökleri zamanlanmış olarak tarıyor, anlık görüntüleri tutuyor ve HTTP üzerinden sunuyor. Tek statik ikili — ve yalnızca okuyor.",
    agentPoints: [
      "Kök başına beş alanlı cron zamanlaması, sayıya göre saklama",
      "Bearer token zorunlu; serve token olmadan başlamayı reddediyor",
      "Varsayılan olarak yalnızca loopback dinliyor — bir envanteri ağa açmak bilinçli bir düzenleme olmalı",
      "/health token istemiyor, böylece konteyner sağlık kontrolü çalışıyor ve başka hiçbir şey açığa çıkmıyor",
      "Bilinmeyen yapılandırma anahtarları sessizce yoksayılmıyor, açılışta reddediliyor",
    ],
    agentCta: "Ajanın tam kurulumu",

    dockerTitle: "Konteynerde",
    dockerNote:
      "Ana makineyi salt okunur bağla ve onu tara. İmaj amd64 ve arm64'ü birlikte taşıyor.",

    openTitle: "Açık kaynak, ve öyle kalıyor",
    openBody:
      "Tarayıcı, anlık görüntü deposu, karşılaştırma ve iki ikili de Apache-2.0. Ajan senin sunucularında çalışıyor, dolayısıyla onu okuyabilmen gerekir. Masaüstü uygulaması ve hub ticari kısım.",
  },

  hub: {
    title: "Hub — spacetrace",
    description:
      "Kendi sunucunda çalışan spacetrace hub'ı: ajanlar anlık görüntü gönderiyor, hub geçmişi tutuyor, neyin büyüdüğünü hesaplıyor ve bir disk dolmadan önce haber veriyor.",
    kicker: "Kendi sunucunda · tek ikili, tek SQLite dosyası · telemetri yok",
    headlineGiven: "Kırk makine, kırk disk.",
    headlineSecond: "Tek sayfa, en kötüsü başta.",
    lede: "Ajanlar kendi makinelerini tarıyor ve anlık görüntüleri buraya gönderiyor. Hub geçmişi tutuyor, neyin büyüdüğünü hesaplıyor ve bir disk dolmadan önce sana söylüyor. İzlediği makinelere hiç dokunmuyor.",
    ctaBinaries: "İkili dosyalar",
    dashCaption:
      "Ekran görüntüsü değil, çizim. Gerçek pano sunucuda oluşturulan HTML — kendi sunucunda çalışan bir aracın sana bir sayfa göstermek için npm install istemesi, onu daha kötü bir araç yapar.",
    withheldNote:
      "Boş bir tahmin, eksik bir sayı değil; tahmin etmeyi reddetmek. Hedefin kendi sayfası hangi koşulun sağlanmadığını yazıyor.",

    setupTitle: "Kurulumu",
    setupLede:
      "İki kimlik bilgisi, bir yapılandırma dosyası ve her ajana eklenen bir satır. Ayağa kaldırılacak başka bir şey yok.",
    step1Title: "Bir admin tokenı ve yapılandırma oluştur",
    step1Body:
      "hub.toml olarak kaydet, sonra docker compose up -d — ya da ikiliyi doğrudan çalıştır ve sunmaya başlamadan önce yapılandırmayı doğrula.",
    step2Title: "Giriş yap ve bir ajan tokenı oluştur",
    step2Body:
      "Hub'ı aç, admin-token içeriğiyle giriş yap ve Agents sayfasından bir token oluştur. Ya da komut satırından yap.",
    step3Title: "Her ajanı ona yönlendir",
    step3Body:
      "Bunu ajanın zamanlamasına ekle, filo kendini dolduruyor. Yeniden göndermek zararsız: aynı sunucu, kök ve başlangıç zamanına sahip bir anlık görüntü varsa atlanıyor.",
    step4Title: "Önüne TLS koy",
    step4Body:
      "Hub'ı kendi ağının dışına açmadan önce TLS'i bir ters vekilde sonlandır. Oturum çerezi HttpOnly ve SameSite=Strict; Secure bayrağı HTTPS üzerinden sunmakla geliyor.",

    credsTitle: "Bilinçli olarak iki kimlik bilgisi",
    credsBody1:
      "Bir NAS'taki yapılandırma dosyasında duran token, bütün filonun envanterinin anahtarı olmamalı. Bu yüzden bir ajan tokenı pano kimlik bilgisi değil — tersi de geçerli.",
    credsBody2:
      "Ajan tokenları SHA-256 özeti olarak saklanıyor, yani veritabanının bir kopyası çalışan kimlik bilgileri kümesi değil. İptal etmek kaydı koruyor ve tokenı anında çalışmaz hâle getiriyor.",
    credsTested: "İki yön de testlerle kapsanıyor.",
    credCol: "Kimlik bilgisi",
    credCan: "Yapabildiği",
    credCannot: "Yapamadığı",
    agentToken: "Ajan tokenı",
    adminToken: "Admin tokenı",
    agentCan: "Anlık görüntü göndermek",
    agentCannot: "Panoyu ya da API'yi okumak",
    adminCan: "Her şeyi okumak, token ve kural yönetmek",
    adminCannot: "Anlık görüntü göndermek",

    forecastTitle: "Tahmin, en sıkı tutulan özellik",
    forecastLede:
      "Yanlış olma olasılığı en yüksek olan o, bu yüzden cevap vermeyi en sık reddeden de o. “N gün içinde dolar” yalnızca bunların hepsi sağlanırsa görünüyor.",
    fc1Title: "En az üç anlık görüntü, en az bir günü kapsayan",
    fc1Body:
      "İki nokta her zaman kusursuz bir doğru verir. Bir gün, günlük bir örüntünün hiç görünebileceği en kısa aralık.",
    fc2Title: "r² ≥ 0,5 doğrusal uyum",
    fc2Body:
      "Disk kullanımı sıklıkla doğrusal değil. Bir log döndürmesi ya da tek seferlik bir geri yükleme, eğimi hiçbir şey ifade etmeyen bir doğru üretmekten memnun olur.",
    fc3Title: "Gerçekten ölçülmüş bir kapasite",
    fc3Body:
      "Dosya sisteminin gerçek boyutu olmadan dolacak bir şey yok. Hub bunu, gördüğü en büyük toplamdan tahmin etmiyor.",
    fc4Title: "On yıl içinde bir cevap",
    fc4Body: "“4.000 gün içinde dolar” aritmetiktir, bilgi değil.",
    fc5Title: "Büyüme ve tahmin farklı sayılar",
    fc5Body:
      "Büyüme taranan klasör üzerinde ölçülüyor. Tahmin, dosya sisteminin kalan alanını o hızda ileriye yansıtıyor. Sayfalar hangisinin hangisi olduğunu söylüyor; ikisini tek bir rahatlatıcı rakama karıştırmıyor.",

    alertsTitle: "Uyarılar",
    alertsLede:
      "Eşik kuralları, webhook ile iletiliyor. Üç tür, her birinde bir bekleme süresi — böylece dolan bir disk aynı mesajın akışına dönüşmüyor:",
    alerts: [
      "Boş alan bir yüzdenin altına düştüğünde",
      "Büyüme bir hızın üstüne çıktığında",
      "Tahmin belli bir ufkun içine girdiğinde",
    ],
    alertsMissing:
      "E-posta ile iletim henüz yapılmadı ve kişi başına giriş yerine tek bir admin kimlik bilgisi var.",

    apiTitle: "HTTP API",
    noRollupTitle: "Neden özet tablosu yok",
    noRollupBody1:
      "Anlık görüntüler, komut satırının ve ajanın kullandığı aynı depoda değişmeden tutuluyor. Hub kendine ait bir özet tablosu tutmuyor; yani geldiği anlık görüntülerle uyumsuz hâle gelebilecek hiçbir şey yok.",
    noRollupBody2:
      "Bir ajanın gönderdiği şey, yerel olarak sakladığı dosyanın aynısı; yani iki tarafta da hiçbir dönüştürme yok ve bir hedefin sayfasındaki karşılaştırma, dizüstünde çalışan aynı kod tarafından hesaplanıyor.",

    fleetHost: "Sunucu",
    fleetRoot: "Kök",
    fleetFree: "Boş",
    fleetGrowth: "Büyüme",
    fleetTrend: "Son 14 gün",
    fleetFills: "Dolma süresi",
    fleetDays: "gün",
    fleetTargets: "hedef",
    fleetReporting: "bildiriyor",
    fleetSilent: "3 gündür sessiz",
    fleetNoCapacity: "kapasite kaydedilmedi",
    fleetTwoSnapshots: "2 anlık görüntü",
  },

  download: {
    title: "spacetrace'i indir",
    description:
      "spacetrace masaüstü uygulaması, komut satırı aracı ve ajanı ile kendi sunucunda çalışan hub için indirmeler ve kurulum talimatları. macOS, Windows, Linux, NAS ve Docker.",
    headline: "İndir",
    lede: "Kurabileceğin üç şey, hepsi aynı çekirdekten. İhtiyacın olanı al; herhangi biriyle yazılan anlık görüntüyü diğerleri okuyabiliyor.",

    detectMac: "macOS",
    detectMacSub: "Universal — Apple silicon ve Intel",
    detectWindows: "Windows",
    detectWindowsSub: "64 bit",
    detectLinux: "Linux",
    detectLinuxSub: "Statik ikililer — kurulacak çalışma zamanı yok",
    detectUnknown: "Bir platform seç",
    detectUnknownSub:
      "Bu tarayıcı hangi makinede olduğunu söylemedi. Aşağıdaki tablolar bütün dosyaları listeliyor.",
    goToFiles: "Dosya listelerine git",
    alsoCli: "Komut satırı aracı ve ajan",
    cliAlongside:
      "Uygulama ve komut satırı aracı tek bir anlık görüntü veritabanını paylaşıyor, yani birlikte kullanılabiliyorlar.",

    continuousTitle: "“continuous” ne demek",
    continuousBody:
      "İlk etiketli sürüme kadar buradaki her indirme main'in en yeni derlemesi: CI'ı geçmiş, başka hiçbir şey değil. Kararlı bir sürüm çıktığında bu sayfa otomatik olarak ona geçiyor.",

    desktopSection: "Masaüstü uygulaması",
    desktopSectionLede:
      "Treemap penceresi. Faz 3'te olduğu sürece indirmesi ücretsiz.",
    desktopCaption:
      "macOS derlemesi universal, yani Apple silicon ve Intel için tek dosya var.",

    cliSection: "Komut satırı ve ajan",
    cliSectionLede:
      "Tek arşiv iki ikiliyi de taşıyor: komut satırı için spacetrace ve zamanlanmış tarama yapıp HTTP üzerinden cevap veren servis için spacetrace-agent.",
    cliCaption:
      "Linux derlemeleri statik musl, yani eski glibc üzerinde ve boş bir konteynerin içinde çalışıyor. Her arşiv README ve lisansı da taşıyor.",

    hubSection: "Hub",
    hubSectionLede:
      "Filo görünümü. Tek ikili, tek SQLite dosyası, veritabanı sunucusu yok, frontend derlemesi yok.",
    hubCaption:
      "Konteyner imajı iki mimariyi de taşıyor, yani docker pull etiket eki olmadan doğru olanı alıyor.",

    unsignedTitle: "Buradaki hiçbir şey imzalı değil",
    unsignedBody:
      "İmzalama sertifikaları yıllık ücretli ve bir public depoda duramıyor; bu yüzden macOS ve Windows ilk seferde ikisi de seni durduruyor. Bu, şüphe duymayı hak eden bir durum — her dosyanın yanında bir sağlama toplamı yayınlanmasının ve derlemenin okuyabildiğin bir iş akışı olmasının sebebi bu.",

    installMacTitle: "macOS: “spacetrace açılamıyor”",
    installMacBody:
      ".dmg dosyasını aç, uygulamayı Applications'a sürükle, sonra sağ tıklayıp Aç'ı seç — çift tıklamanın vermediği tek seferlik izni bu veriyor. Ya da karantina bayrağını doğrudan temizle:",
    installMacFda:
      "Ana dizininin dışını taramak için uygulamaya Full Disk Access gerekiyor: Sistem Ayarları → Gizlilik ve Güvenlik.",
    installWinTitle: "Windows: SmartScreen",
    installWinBody:
      "“Windows bilgisayarınızı korudu” → Daha fazla bilgi → Yine de çalıştır. Kurulum kullanıcı düzeyinde ve yönetici hakkı istemiyor.",
    installLinuxTitle: "Linux",
    installLinuxBody:
      "AppImage'in hiç kurulmaya ihtiyacı yok: çalıştırılabilir işaretle ve çalıştır. Wayland'de pencere boş geliyorsa WEBKIT_DISABLE_DMABUF_RENDERER=1 ile çalıştır — bu bir WebKitGTK sorunu, uygulamanın değil.",

    verifyTitle: "İndirdiğini kontrol et",
    verifyBody:
      "Her sürüm, kendi varlıklarını kapsayan bir SHA256SUMS dosyası taşıyor. Kurmadan önce doğrula — özellikle ikililerin hiçbiri imzalı olmadığı için.",
    verifyMacNote: "macOS'ta sha256sum yok; yerine shasum -a 256 -c kullan.",

    buildTitle: "Ya da kendin derle",
    buildBody:
      "Çekirdek, komut satırı aracı ve ajan Apache-2.0 ve yalnızca Rust 1.85 veya üstünü gerektiriyor.",
    buildNote:
      "Masaüstü uygulaması ve hub açık kaynak değil, dolayısıyla kaynaktan derlenemiyor — çekirdeğin izin verici lisansta kalmasının karşılığı bu.",

    platMacArm: "macOS, Apple silicon",
    platMacIntel: "macOS, Intel",
    platMacAny: "macOS 11+",
    platWin: "Windows 10 / 11",
    platLinuxX64: "Linux, x86_64",
    platLinuxArm: "Linux, aarch64",
    platDebian: "Debian, Ubuntu",
    platFedora: "Fedora, RHEL",
    platAnyLinux: "Herhangi bir Linux",
    noteUniversal: "Universal ikili",
    noteNsis: "NSIS kurulumu, kullanıcı düzeyinde",
    noteDeb: "libwebkit2gtk-4.1 gerekiyor",
    noteRpm: "webkit2gtk4.1 gerekiyor",
    noteAppImage: "Çalıştırılabilir işaretle ve çalıştır; hiçbir şey kurulmuyor",
    noteStatic: "Statik; sunucular ve konteynerler",
    noteNas: "Raspberry Pi, ARM sunucular, çoğu NAS",
    noteM1: "M1 ve sonrası",
    noteMac11: "macOS 11+",
    noteWinAlloc:
      "Burada diskteki boyut mantıksal boyuta eşit; bilinen eksiklere bak",
    noteAllArchives: "Yukarıdaki her arşivi kapsayan tek dosya",
    noteHubServer: "Statik; bir sunucu için olağan seçim",
    noteHubTry: "Önce kendi makinende denemek için",
  },

  guide: {
    title: "Kılavuz — spacetrace",
    description:
      "spacetrace nasıl kullanılır: tarama, anlık görüntü tutma, ikisini karşılaştırma, sunucuya ajan kurma, uzak bir makineyi okuma ve hangi boyut ölçüsünün ne anlama geldiği.",
    headline: "Kılavuz",
    lede: "Komut satırı aracının yaptığı her şey, ihtiyaç duyacağın sırayla. Herhangi bir komutta --help aynı şeyi tarayıcı olmadan basıyor.",
    onThisPage: "Bu sayfada",

    startTitle: "İlk tarama",
    startBody:
      "Tarama bir klasörü yürüyor ve bulduğunu basıyor. Hiçbir şey saklanmıyor, hiçbir şey değişmiyor.",
    startUnreadable:
      "Okunamayan yollar yürüyüşü durdurmak yerine sayılıyor ve örnekleniyor. macOS'ta ana dizinin dışına uzanmak için terminaline Full Disk Access gerekiyor; o olmadan da tarama tamamlanıyor ve neyi kaçırdığını söylüyor.",
    startNarrowTitle: "Daha hızlı ve daha dar yapmak",
    startExclude:
      "--exclude bir yol değil bir dizin adı alıyor ve tekrarlanabiliyor. Hariç tutulan klasörlerin içine hiç inilmiyor; yani bir geliştirici makinesinde node_modules'ü hariç tutmak genellikle iki saniye ile otuz saniye arasındaki fark.",

    snapTitle: "Anlık görüntüler ve karşılaştırma",
    snapBody:
      "Diğer analizcilerde olmayan kısım bu. Bir taramayı sakla, sonra bir tane daha sakla ve aralarında neyin değiştiğini sor.",
    snapThen: "Sonra karşılaştır. Argüman verilmezse son ikisini alıyor.",
    culpritTitle: "Neden en derindeki klasörü söylüyor",
    culpritBody:
      "Büyümesi tamamen tek bir çocuğundan gelen bir klasör sana bilmediğin hiçbir şey söylemiyor, o yüzden atlanıyor. Rapor, değişimin gerçekten dağıldığı ilk seviyeyi söylüyor — suçluyu, atalarını değil.",
    snapSinceTitle: "Diskin şu anki hâline karşı",
    snapSinceBody:
      "İki anlık görüntüye ihtiyacın yok. En yenisini canlı bir taramayla karşılaştır:",
    snapFilterTitle: "Neyin görüneceğini seçmek",
    snapPruneTitle: "Veritabanının sonsuza kadar büyümesini engellemek",

    browseTitle: "Bir taramayı gezmek",
    browseBody:
      "ls klasörleri boyuta göre listeliyor; ya canlı bir taramadan ya da saklanmış bir anlık görüntüden.",
    browseExport:
      "Bir anlık görüntüyü başka bir şeye vermek için export ncdu'nun kendi biçimini yazıyor:",
    browseJson:
      "Her komut ayrıca --json alıyor; betikleme için desteklenen yol bu. İnsan tarafından okunan kolonlar değişmekte serbest, JSON şekli değil.",

    agentTitle: "Sunucuda ajan",
    agentBody:
      "spacetrace-agent aynı kodun servis hâli: yapılandırdığın kökleri zamanlanmış olarak tarıyor, anlık görüntüleri tutuyor ve HTTP üzerinden cevap veriyor. Tek statik ikili, ve yalnızca okuyor — içinde kendi anlık görüntü veritabanının dışında bir şeyi silen hiçbir kod yolu yok.",
    agentStep1: "Bir yapılandırma ve token yaz",
    agentStep1Body:
      "serve token olmadan başlamayı reddediyor. Kimlik doğrulaması olmayan bir ajan, bütün dosya sistemi envanterini o porta ulaşabilen herkese teslim eder.",
    agentStep2: "Neyi ne zaman tarayacağını söyle",
    agentStep2Body:
      "Beş cron alanı; *, a-b, */n ve listelerle. Saniye yok, @daily yok. Bilinmeyen yapılandırma anahtarları açılışta reddediliyor, çünkü kimsenin izlemediği bir makinede sessizce hiçbir şey yapmayan bir yazım hatası, açılmayı reddetmekten kötüdür.",
    agentStep3: "Başlatmadan önce kontrol et",
    agentStep3Body:
      "Hiç tetiklenemeyecek bir zamanlama sessizce başarısız olmak yerine never diyor.",
    agentStep4: "Çalıştır",
    agentStep4Body:
      "Birlikte gelen systemd birimi ajanı kendi yetkisiz kullanıcısı olarak ProtectSystem=strict altında, Nice=10 ve boşta I/O önceliğiyle çalıştırıyor. Bir tarama, makinenin asıl işine hiç engel olmamalı.",
    agentDocker: "Docker'da ana makineyi salt okunur bağla ve onu tara:",
    agentSecTitle: "Dışa açmadan önce",
    agentSec: [
      "Varsayılan olarak loopback dinliyor. Bir dosya sistemi envanterini ağa yayınlamak bilinçli bir düzenleme olmalı.",
      "Ajanda TLS yok. Önüne bir ters vekil koy.",
      "Token erken çıkış olmadan karşılaştırılıyor; yani yanlış bir token, ne kadarı doğru olursa olsun aynı sürede reddediliyor.",
      "/health token istemiyor, böylece konteyner sağlık kontrolü çalışıyor; yalnızca durum ve sürüm dönüyor — sunucu adı yok, kök yok.",
      "Anlık istek üzerine tarama kapalı. Açıkken, tokenı elinde tutan herkes ajanın kullanıcısının okuyabildiği her dizini listeleyebilir.",
    ],

    remoteTitle: "Başka bir makineyi okumak",
    remoteBody:
      "Salt okunur her komut --remote alıyor. Yeni bir şey öğrenmek gerekmiyor: aynı alt komutlar, başka yere yönlenmiş.",
    remoteSaveBody:
      "Adres ve tokenın yazılmaktan çıkması için bir uzak kaynak kaydet — ~/.config/spacetrace/remotes.toml içinde:",
    remoteNotDifferentTitle: "Uzak bir anlık görüntü farklı bir tür şey değil",
    remoteNotDifferentBody:
      "Telden inen şey, ajanın sakladığı aynı bağımsız SQLite dosyası; yani listelemek, gezmek ve karşılaştırmak yerel bir anlık görüntüyle birebir aynı kodu çalıştırıyor. Aracı tamamen atlayıp yine açabileceğin bir dosya alabilirsin:",
    remoteRefuse:
      "scan, prune ve rm --remote ile çalışmayı reddediyor: yerel durum üzerinde iş yapıyorlar ve ajan hiçbir şeyi silmiyor.",

    sizeTitle: "Hangi boyut, ve neden",
    sizeBody: "Her girdi iki sayı taşıyor ve hiçbiri diğerinin tahmini değil.",
    sizeMeasure: "Ölçü",
    sizeWhat: "Ne olduğu",
    sizeMatches: "Neyle birebir",
    sizeLogical: "mantıksal",
    sizeLogicalWhat: "Her dosyanın bildirdiği uzunluk",
    sizeOnDisk: "diskte",
    sizeOnDiskWhat: "Gerçekten tahsis edilen bloklar, dizin blokları dâhil",
    sizeCapacity: "kapasite",
    sizeCapacityWhat: "Taranan dosya sisteminde toplamın ne kadarının boş olduğu",
    sizeCapacityMatches: "df'in Avail kolonu, birebir",
    sizeDiverge:
      "İki yöne de ayrışıyorlar ve ikisi de doğru. Bir baytlık dosya bütün bir blok tahsis ediyor, yani diskte uzunluğundan büyük. Seyrek bir dosya hiç tahsis etmediği bir uzunluk bildiriyor: bir disk imajı bir terabayt iddia edip on dokuz gigabayt tutabiliyor.",
    sizeWhyOnDisk:
      "Uygulamanın varsayılanının diskte olmasının sebebi seyrek dosyalar. Sanal makine imajları, veritabanı dosyaları ve çekirdek dökümleri gerçek her diskteki en büyük girdiler arasında; yani mantıksal ölçü tam da en çok önemli olan girdilerde en çok yanılıyor. Komut satırının varsayılanı mantıksal ve ikisi de hangisini gösterdiğini söylüyor.",
    sizeCapacityWhy:
      "Kapasite “% dolu” olarak değil, toplamın boşu olarak raporlanıyor. Alanı birimler arasında paylaşılan bir dosya sisteminde — APFS konteyneri, btrfs alt birimleri, ince LVM — kullanılan bir rakam kardeşleri de içerir ve aynı bağlama noktası için df ile çelişir.",
    sizeLinks:
      "Sabit bağlantılar varsayılan olarak bir kez sayılıyor; ikinci kopya ağaçta sıfır bayt katkıyla görünüyor. Sembolik bağlantılar hiç izlenmiyor ve kendi boyutlarıyla sayılıyor.",

    refTitle: "Komut başvurusu",
    refOptionsTitle: "Çoğu komutta çalışan seçenekler",

    storageTitle: "Neyin nerede tutulduğu",
    storagePlatform: "Platform",
    storageDb: "Anlık görüntü veritabanı",
    storageAnywhere: "Her yerde",
    storageOverride:
      "SPACETRACE_HOME bunu geçersiz kılıyor; --db ise tek bir komut için onu geçersiz kılıyor",
    storageShared:
      "Masaüstü uygulaması aynı veritabanına yazıyor; yani uygulamada kaydedilen bir anlık görüntü spacetrace scans içinde görünüyor, tersi de geçerli.",

    limitsTitle: "Bilinen eksikler",
    limitsLede:
      "Keşfedilmesi beklenmeden yayınlandı. Bunlardan birine denk gelirsen bilinen borç, sürpriz değil.",
    limits: [
      "Windows'ta diskteki boyut mantıksal boyuta eşit ve sabit bağlantı tekilleştirmesi kapalı. Gerçek rakamlar için GetFileInformationByHandleEx ve FileIdInfo gerekiyor.",
      "APFS klonları tekilleştirilmiyor; btrfs veya ZFS'te reflink ve sıkıştırma yüzünden bir ağaç yürüyüşü gerçek kullanımı raporlayamıyor. Hiçbir ağaç yürüyücüsü yapamaz; sayılar dosyaların kendi sayıları.",
      "Tarama bütün ağacı bellekte tutuyor. On milyon dosyanın üstündeki bellek profili ölçülmedi.",
      "Ajanda TLS ve hız sınırlama yok. Ters vekil kullan; token zaten zorunlu.",
      "Zamanlayıcıda saat dilimi veritabanı yok — UTC artı sabit bir offset, yani yaz saati kaymalarını düşünmek sana kalıyor.",
    ],
  },

  footer: {
    licence:
      "Tarayıcı, anlık görüntü deposu, komut satırı aracı ve ajan Apache-2.0. Masaüstü uygulaması ve hub ticari.",
    home: "Ana sayfa",
    allReleases: "Bütün sürümler",
    builtWith: "Astro ile yapıldı. İzleyici yok, çerez yok.",
  },
};
