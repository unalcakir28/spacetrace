# Yapılacaklar

Canlı çalışma listesi. Faz tanımları ve çıkış kriterleri için
[docs/ROADMAP.md](docs/ROADMAP.md), gerekçeler için [docs/WHY.md](docs/WHY.md),
rakiplerin nerede önde olduğu için [docs/COMPETITORS.md](docs/COMPETITORS.md).

Son güncelleme: 9 Eylül 2026 (**Sıra 1–3 kapandı** — E1, E2, A4, A1, A2, A4w,
B1. Üç platformda CI yeşil. B1'in kalan maddesi **B1-K** ayrı bir oturumda
Fable modeliyle derin araştırmaya ertelendi. Sıradaki iş → Sıra 4: A3, A5)

---

## Açık kararlar ✅ kapandı

Beşi de 7 Eylül 2026'da karara bağlandı. Gerekçeler ve ölçümler
[docs/DECISIONS.md](docs/DECISIONS.md) içinde; özet:

- [x] **Arayüz dili** → İngilizce (K1). CLI, README, ARCHITECTURE çevrildi;
      gerekçe belgeleri Türkçe kaldı. i18n kapsam dışı ilan edildi.
- [x] **Lisans modeli** → çekirdek + ajan Apache-2.0 bu monorepo'da; masaüstü ve
      merkez ayrı depoda ticari (K2). WHY.md'deki "Pro = sınırsız ajan"
      hipotezi uygulanamaz olduğu için düzeltildi.
- [x] **Ajan protokolü** → HTTP + JSON, çatı axum; snapshot gövdesi
      `application/octet-stream` (K3).
- [x] **Snapshot taşınabilirliği** → ham SQLite, `VACUUM INTO` + zstd (K4).
      Ölçüldü: girdi başına 49.5 B ham, 12.4 B zstd → 1M dosya ≈ 12 MB.
- [x] **GitHub deposu** → public (K5).

---

## Faz 1 — Çekirdek ve CLI ✅

- [x] Workspace iskeleti, CI (ubuntu/macos/windows), Apache-2.0
- [x] `scan-core`: paralel DFS, arena ağaç (BFS düzeni, bitişik çocuklar)
- [x] Hardlink tekilleştirme `(dev, ino)`, `--no-dedupe` ile kapatılabilir
- [x] Sembolik bağlantılar izlenmiyor, kendi boyutlarıyla sayılıyor
- [x] `alloc` = `st_blocks * 512`; `size` = yalnızca dosya baytları
- [x] `--exclude`, `-x/--one-file-system`, `--depth`
- [x] İzin hatalarını sayma ve örnekleme (tarama durmuyor)
- [x] `store`: SQLite şeması, arena'yı olduğu gibi saklama, `PRAGMA user_version`
- [x] `store`: `list`, `latest_for`, `last_two_for`, `delete`, `prune`
- [x] ncdu uyumlu JSON dışa aktarım
- [x] `diff`: suçlu klasör tespiti (yoğunlaşma eşiği), eklenen/silinen alt ağaçlar
- [x] `cli`: scan / ls / scans / diff / export / prune / rm
- [x] Canlı ilerleme göstergesi, her komutta `--json`
- [x] 32 test; `du` ile birebir doğrulama (`/usr`, `/usr/share`, `/etc`)
- [x] clippy uyarısız, `cargo fmt` temiz, Windows hedefi tip denetimi
- [x] README, WHY, ROADMAP, ARCHITECTURE

---

## Faz 2 — Ajan ✅ çekirdek tamam

### Çekirdek
- [x] `agent` crate'i (workspace'e eklendi, `spacetrace-agent` ikilisi)
- [x] `agent scan` — tek seferlik, ağsız (`--root` ile tek kök)
- [x] Yapılandırma dosyası (TOML): kökler, hariç tutulanlar, zamanlama, token
      — bilinmeyen anahtarlar reddediliyor, `agent check` ile ön doğrulama
- [x] Dahili zamanlayıcı — elle yazılmış 5 alanlı cron (chrono eklenmedi),
      Vixie'nin dom/dow birleşim kuralı dâhil
- [x] Snapshot rotasyonu (kök başına `keep`; `store::prune_target`)

### Ağ
- [x] `agent serve` — axum HTTP servisi
  - [x] `GET /health` (tokensiz, yalnızca canlılık+sürüm), `GET /status`
  - [x] `GET /scans`, `GET /scans/:id`, `GET /scans/:id/download`
  - [x] `POST /scans` (202 + arka planda tarama), `POST /snapshots` (alıcı uç)
  - [x] Bearer token doğrulama (sabit zamanlı karşılaştırma), dosya/env/config
  - [x] Gövde boyutu sınırı, SIGTERM ile temiz kapanma
  - [ ] Opsiyonel yerleşik TLS — şimdilik ters vekil öneriliyor
- [x] `agent push <url>` — snapshot'ı merkeze/başka ajana gönder (zstd)
- [x] Eşzamanlı tarama kilidi (aynı kök iki kez taranmıyor → 409)
- [ ] Hız sınırlama — token zaten gerekli olduğu için ertelendi

### İstemci tarafı
- [x] CLI'da uzak kaynak: `--remote <url|ad>` (scans / ls / diff / export)
- [x] `spacetrace pull` — uzak snapshot'ı yerel veritabanına al
- [x] Uzak kaynak tanımları `remotes.toml`
- [ ] SSH modu: kurulum gerektirmeden karşı tarafta geçici ajan çalıştırma

### Dağıtım
- [x] Statik ikili (musl) — linux/amd64, linux/arm64 (release workflow)
- [x] Docker imajı (host'u read-only mount ile tarar)
- [x] systemd unit + örnek yapılandırma (sertleştirilmiş, ProtectSystem=strict)
- [x] `curl | sh` kurulum betiği (POSIX sh, busybox uyumlu)
- [x] GitHub Releases otomasyonu (tag → build → artefakt + SHA256SUMS)

### Doğrulama (çıkış kriteri)
- [x] Uçtan uca yerel doğrulama: ajan + CLI `--remote diff` anlamlı çıktı
      veriyor, suçlu klasörü doğru buluyor
- [ ] **Kendi Hetzner sunucularına, Proxmox host'una ve bir konteynere kur**
- [ ] **Bir hafta gerçek veri topla** — bu ikisi yalnızca gerçek makinelerde
      yapılabilir, kod tarafı hazır

---

## Faz 3 — Masaüstü ✅

Ayrı depo: [spacetrace-desktop](https://github.com/unalcakir28/spacetrace-desktop) (K2).
Yerleşim motoru burada kaldı (`crates/treemap`), çünkü çekirdek ve test edilebilir.

- [x] Tauri v2 + React/TS iskeleti, Rust çekirdeğini süreç içinde çağırma
- [x] Squarified treemap — yerleşim Rust'ta (`crates/treemap`), çizim Canvas2D
- [x] LOD: min_area altındaki dikdörtgenler bölünmüyor; karo sayısı diskin
      değil ekranın büyüklüğüne bağlı
- [x] Culling ve hit-test — quadtree yerine hiyerarşinin kendisi kullanıldı
      (çocuk her zaman ebeveyninin içinde olduğu için ayrı indeks gereksiz)
- [x] Klasör ağacı paneli (tembel genişleyen), çift yönlü seçim senkronu
- [x] Dosya türüne göre renklendirme, çöpe gönder, Finder/Explorer'da aç
- [x] Uzak kaynak akışı: ajandan snapshot indir, yerelmiş gibi gez
- [x] Diff görünümü (iki snapshot karşılaştırma tablosu)
- [x] Düğüm kimlikleri generation'a bağlı — eski ağaca ait id reddediliyor
- [ ] **Windows MFT hızlı yolu** (`usn-journal-rs`) — yönetici arkasında
- [ ] macOS Full Disk Access onboarding ekranı
- [ ] Zaman çizelgesi görünümü (bir hedefin tüm geçmişi)
- [ ] Üç WebView'da treemap performans testi (WebKitGTK dâhil) — yalnızca
      macOS'ta doğrulandı

---

## Faz 4 — Merkez ✅

Ayrı depo: [spacetrace-hub](https://github.com/unalcakir28/spacetrace-hub) (K2).

- [x] axum + SQLite servisi, self-host, tek statik ikili
- [x] Çoklu ajan panosu — aciliyete göre sıralı (önce dolacak olan)
- [x] Klasör başına büyüme trendi (en küçük kareler, uyum kalitesiyle birlikte)
- [x] "Bu hızla giderse N gün sonra dolar" tahmini — dayanağı zayıfsa
      söylenmiyor (≥3 örnek, ≥1 gün, r² ≥ 0.5, ölçülmüş kapasite, ≤10 yıl)
- [x] Eşik uyarıları: webhook (boş yer, büyüme hızı, dolma ufku) + cooldown
- [x] Ekip erişimi ve token yönetimi — ajan token'ları hash'li ve iptal
      edilebilir; ajan token'ı panoyu okuyamaz, admin token'ı push edemez
- [x] docker-compose ile tek komut kurulum
- [x] Kapasite ölçümü çekirdeğe eklendi (şema v2) — tahminin ön koşulu
- [ ] E-posta ile uyarı (şimdilik yalnızca webhook)
- [ ] Kişi başına hesap (şimdilik tek admin kimliği)

---

## Rekabet açıkları

9 Eylül 2026 rakip analizinden çıkan iş listesi. Gerekçeler, ölçümler ve
kaynaklar [docs/COMPETITORS.md](docs/COMPETITORS.md) içinde; her madde **hangi
rakibin bizden iyi olduğuyla** etiketli. Sıralama aşağıda, "Sıra" başlığında.

### A. Doğruluk — iddiamızı üç platformda karşıla

Bunlar eksik özellik değil, **verdiğimiz sözü tutmama**. Hız eksiği rekabetçi
dezavantaj; yanlış rakam ürünün kendisini çürütür.

- [x] **A1 Windows `alloc` gerçek değeri** — yazıldı *(9 Eylül 2026)*,
      **CI onayı bekliyor** (macOS'ta yalnızca tip denetimi yapılabiliyor).
      `FILE_STANDARD_INFO` → **`AllocationSize`** kullanılıyor.
      *Önce `GetCompressedFileSizeW` denendi ve CI yanlışladı:* sıkıştırılmamış
      ve sparse olmayan dosyalarda mantıksal boyutu döndürüyor — 100.001
      baytlık dosyaya 100.001 dedi. Adı zaten bunu söylüyormuş. Yani yol
      tabanlı bir çağrıyla olmuyor, handle şart. Dizinler de sorgulanıyor ki
      `alloc` iki platformda aynı şeyi anlatsın.
      *Rakip:* TreeSize, WizTree, WinDirStat 2.5.0 doğru rakam veriyor.
- [x] **A2 Windows hardlink dedupe** — yazıldı *(9 Eylül 2026)*, **CI onayı
      bekliyor**. `GetFileInformationByHandle` tek çağrıda `nNumberOfLinks` +
      `nFileIndexHigh/Low` + `dwVolumeSerialNumber` veriyor, yani nlink, ino ve
      dev birlikte geliyor. Handle `CreateFileW` yerine `std::fs::OpenOptions`
      ile açılıyor (`FILE_READ_ATTRIBUTES`, `BACKUP_SEMANTICS`,
      `OPEN_REPARSE_POINT`): RAII kapatıyor, erken dönüşte sızma yok, unsafe
      yüzeyi küçük ve **yeni windows-sys feature'ı gerekmedi**.
      *Yan kazanç:* `-x/--one-file-system` Windows'ta artık gerçekten çalışıyor
      — daha önce her girdi volume 0 bildirdiği için sessizce etkisizdi.
      *Maliyet:* dosya başına bir handle. Yalnızca dedupe veya `-x` açıkken
      ödeniyor (`FileIdentity::Skipped`); ölçülmüş mertebe +36%, kaldıran B4.
      *Rakip:* WinDirStat 2.5.0 (Ocak 2026).
- [ ] **A3 APFS clone tekilleştirme** — macOS'ta `alloc` şişiyor.
      *Rakip:* **DaisyDisk 4.34** — clone'un yalnızca ilk görünümünü sayıp
      kalanlarına 0 bayt veriyor. macOS en güçlü platformumuz ve orada yanlışız.
- [x] **A4 (Unix) `du` karşılaştırma testi — yazıldı.** *(9 Eylül 2026)*
      **Madde yanlış kurulmuştu:** "test yalnızca macOS'ta koşuyor" değil,
      **test hiç yoktu**. `totals_match_the_files_on_disk` testin kendi yazdığı
      sabitlerle karşılaştırıyordu ve `alloc` iddiası `>= 4096`'ydı; `du`
      doğrulaması elle yapılıyordu. Buna karşılık CLAUDE.md, ARCHITECTURE.md ve
      WHY.md "bu bir test koşulu" diyordu — üçü de artık doğru.
      Yeni: `crates/scan-core/tests/du_equivalence.rs`, 6 test, CI'da
      ubuntu + macos'ta koşuyor. `alloc` oracle'ı harici `du`; `size` oracle'ı
      aynı dosyadaki **naif seri yürüyüş**, çünkü `du` mantıksal boyutu
      veremiyor (BSD `-A` bloğa yuvarlıyor, GNU `--apparent-size` dizin
      inode'unu ekliyor — bizim `size` eklemiyor).
      Mutasyon testiyle doğrulandı: `alloc`=`size` → 3 test düşüyor,
      dedupe bozulunca → 3 test, dizin inode'u toplama eklenince → 1 test
      (yalnızca naif yürüyüş yakalıyor; `du` o mutasyona onay verirdi).
- [x] **A4w `du` karşılaştırmasının Windows karşılığı** — yazıldı
      *(9 Eylül 2026)*, A1/A2 ile aynı commit'te.
      `crates/scan-core/tests/windows_metadata.rs`, 6 test.
      Windows'ta `du` yok, harici oracle yok; yerine **inşa tabanlı** test.
      Cluster boyutu taşınabilir biçimde sorulamadığı için hile şu: **512'nin
      katı olmayan bir uzunluk** seçiliyor (100.001). NTFS cluster'ı en az 512
      bayt olduğundan gerçek bir tahsis rakamı bu uzunluğa **eşit olamaz** —
      yani `alloc != size` iddiası, mantıksal boyutun döndürülmediğini cluster
      boyutunu bilmeden kanıtlıyor. Dosya içeriği bilinçli olarak
      **sıkıştırılamaz** (sıfırlarla dolu bir dosya, sıkıştırma açık bir
      birimde testi haksız yere düşürürdü).
      `stats.errors == 0` iddiası da kanarya: sistematik bir API hatası
      olsaydı `alloc` sessizce mantıksal boyuta düşerdi, test bunu yakalar.
- [ ] **A5 Snapshot bütünlük kontrolü** — `export_snapshot` / `import_snapshot`
      ve `push` yolunda checksum yok. Ağ üzerinden bozulan bir bayt doğruluk
      iddiamızı sessizce çürütür. `Tree::from_parts_checked` arena *yapısını*
      doğruluyor (panic ve sonsuz döngü koruması), bit bozulmasını değil.
      *Rakip:* dua-cli v2.44.0 snapshot'ları SHA-256 ile doğruluyor.
- [ ] **A6 btrfs/ZFS farkındalığı** — reflink ve sıkıştırma yüzünden ağaç
      yürüyüşü yanlış. Uzun vade; doğrusu örnekleme gerektiriyor.
      *Rakip:* btdu (Monte Carlo, ~100 örnekte %1 çözünürlük).

### B. Hız — ölçülmüş açıklar

- [ ] **B1 Bellek** — *(9 Eylül 2026: ölçüldü, parçalandı, dördü yapıldı.
      Kalan tek madde **B1-K**, derin araştırmaya ertelendi.)*

      **Dağılım — düzeltme öncesi** (`/Applications`, 412.232 girdi, tepe RSS
      119 MB = **290 B/girdi**), faz faz RSS probuyla ölçüldü:

      | Kalem | MB | B/girdi |
      |---|---|---|
      | taban (ikili + çalışma zamanı) | 8 | — |
      | `RawEntry` ara ağacı (yürüyüş fazı) | 36,3 | 88 |
      | ad `String`'leri | ~13,2 | ~32 |
      | parçalanma, malloc başlıkları, geçici `PathBuf`'lar | ~19 | ~46 |
      | arena (`Node` 104 B) | 42,9 | 104 |

      **Sonuç (aynı gün):** 298 → **231 B/girdi** (117 → 91 MB), yani **%22,5**.
      Hız gerilemedi — dönüşümlü A/B ölçümünde 8 thread'te en iyi 0.976s →
      0.894s, yani hafifçe **hızlandı** (girdi başına bir malloc eksildi).
      Not: ardışık ölçüm önce %12 gerileme göstermişti; makine ısınmasından
      kaynaklanan sürüklenmeydi, dönüşümlü koşturma bunu eledi.

      **Kritik gözlem:** yürüyüş bittiğinde 77 MB, flatten bittiğinde 119 MB.
      Yani `RawEntry` ağacı ile arena **aynı anda yaşıyor** ve `RawEntry`
      serbest bırakılsa da işletim sistemine geri verilmiyor. 290 B/girdinin
      192'si bu **çift depolama**.

      - [x] **Arena kapasitesini önceden ayır.** Girdi sayısı flatten anında
            zaten tam biliniyor (`progress.files + dirs`). Öncesinde 1024'ten
            ikiye katlanarak 524.288'e çıkıyordu. Ölçülen kazanç 298 → 290 B
            (%3) — tahmin ettiğimden çok azdı, çünkü tepe flatten'da değil
            yürüyüşte oluşuyor.
      - [x] **Paylaşılan ad arenası.** Adlar `Tree` içinde tek bir `String`'de;
            düğüm `(offset: u32, len: u16)` tutuyor. `u8` değil `u16`, çünkü
            kökün adı tam yol ve 255 baytı aşabiliyor.
            `Node.name` alanı kalktı, yerine `Tree::name(id)`; düğüm kurmanın
            tek yolu artık `TreeAssembler` + `StoredNode`, yani **hatalı bir
            offset yapı gereği kurulamıyor**. Şema değişmedi (SQLite hâlâ
            satır başına TEXT saklıyor).
      - [x] **Alan daraltma:** `nlink`, `files`, `dirs` `u64` → `u32`.
            `Node` 104 → **72 B** (ölçüldü). `mtime` `i64` kaldı — 1970 öncesi
            dosyalar gerçek ve negatif damga taşıyorlar.
      - [x] **`RawEntry` 88 → 48 B.** Adlar dizin başına tek tamponda
            (`Children { names, entries }`), çocuklar `Option<Box<Children>>`.
            `to_string_lossy()` geçerli UTF-8'de ödünç döndürdüğü için girdi
            başına `String` tahsisi tamamen kalktı: **412k → 35k tahsis.**
      - [ ] **B1-K Çift depolamayı kaldır** — kalan asıl kazanç, **derin
            araştırmaya ertelendi** (bkz. aşağıdaki not).

      **Hedef düzeltmesi:** RESEARCH.md'deki **~25 B/dosya** hedefi bizim alan
      kümemizle **ulaşılabilir değil** ve karşılaştırma elmayla armut. ncdu 2
      düğüm başına `own_size`/`own_alloc`/`files`/`dirs` tutmuyor. Bizim
      taban aritmetiğimiz: en agresif daraltmayla `Node` 72 B + ad ~21 B =
      **~93 B/girdi**, artı çift depolama. Gerçekçi hedef **dua-cli'nin 64 B
      arena düğümü** mertebesi, 25 değil.
      *Rakip:* dua-cli 64 B arena düğümü + paylaşılan ad deposu, RSS %49 aşağı;
      ncdu 2 dosyada 25 B, dizinde 56 B.

      ### B1-K — çift depolama, derin araştırmaya ertelendi

      **Karar (9 Eylül 2026):** kalan iş sıradan bir optimizasyon değil, bir
      mimari karar. Ayrı bir oturumda **Fable modeliyle** derinlemesine
      araştırılacak; o araştırma bitmeden kod yazılmayacak.

      **Problem tanımı.** Tepe bellek 231 B/girdi ve bunun **192'si çift
      depolama**: yürüyüşün ürettiği `RawEntry` ara ağacı (48 B) ile arena
      (`Node` 72 B) aynı anda yaşıyor, üstüne serbest bırakılan ara ağaç
      belleği işletim sistemine geri dönmüyor (arena tek parça büyük bir
      tahsis istiyor, boşalan 48 baytlık parçalar ona yaramıyor).

      **Neden kolay değil.** Yürüyüş paralel DFS üretiyor, arena BFS düzeni
      istiyor (değişmez #2: çocuklar bitişik, her çocuğun indeksi
      ebeveyninden büyük). İki düzen arasında bir ara yapı kaçınılmaz
      görünüyor. Bilinen tek kökten çözüm seviye-senkron BFS ve o da
      iş-çalan DFS'in **ölçülmüş 5.2× kazancını** riske atıyor — yani
      savunabildiğimiz tek hız iddiasını.

      **Araştırmanın cevaplaması gerekenler:**
      - Çift depolama, değişmez #2 korunarak kaldırılabilir mi?
      - Seviye-senkron BFS derin ve dar ağaçlarda paralelliği ne kadar
        kaybediyor? (Derin/dar en kötü durum; `/Applications` gibi geniş
        ağaçlar iyimser örnek.)
      - Ara yapı kaldırılamıyorsa, arena'nın tahsisini ara yapının boşalttığı
        belleği kullanacak biçimde kurmak mümkün mü (arena chunk'lı olsun,
        tek parça olmasın)? Bu, değişmez #2'yi bozmadan çift depolamanın
        yarısını geri kazanabilir.
      - dua-cli 64 B'a nasıl indi ve ara yapı sorununu nasıl çözdü?
        (Kaynak kodu MIT, okunabilir — `[bulunamadı]` değil, okunmadı.)

      **Araştırma öncesi zorunlu:** tekrarlanabilir bir benchmark koşumu.
      Bugün ardışık ölçüm %12'lik **sahte** bir gerileme gösterdi; iki ikiliyi
      dönüşümlü koşturunca tersi çıktı. Bellek **ve** hız birlikte, dönüşümlü
      ve medyanlı ölçülmeden hiçbir tasarım kabul edilmeyecek.

- [ ] **B2 Thread sayısı ayarı** — 16 thread'te 8'e göre **gerileme ölçüldü**
      (1.27 s vs 1.11 s, 412k girdi). Şu an hiç ayar yok, rayon varsayılanı
      (çekirdek sayısı) kullanılıyor — yani varsayılan en iyisi değil.
      *Rakip:* erdtree ampirik 3 thread; TreeSize CPU yüküne göre ayarlıyor.
- [ ] **B3 HDD / ağ sürücüsü modu** — dönen diskte ve NFS'te paralel yürüyüş tek
      thread'den kötü olabilir (seek thrash); bu durum için hiçbir şeyimiz yok.
      *Rakip:* gdu `--sequential`; QDirStat girdileri stat etmeden önce inode'a
      göre sıralıyor.
- [ ] **B4 Windows MFT hızlı yolu** (`usn-journal-rs`) — yönetici gerekiyor,
      ReFS'te ve ağ/FAT'te yok → normal yola geri düşme şart, ve o yol A1/A2'de
      düzeliyor. **Sıra bu yüzden A'dan sonra.**
      *Rakip:* WizTree (ham MFT), TreeSize Free (yönetici), WinDirStat 2.5.0.
- [ ] **B5 macOS `getattrlistbulk` hızlı yolu** (`getattrlistbulk-rs`).
      *Ölçüm (dış):* `dumac` bununla `du`'dan 6.39× hızlı. **Listedeki tek
      "öne geçme" maddesi** — rakiplerin hiçbiri macOS'ta bunu yapmıyor.
- [ ] **B6 Linux `getdents64` + `statx` hızlı yolu.**
      *Rakip:* `dut` sıcak cache'te `du`'dan 6.87×, dust/dua/gdu'dan 2.8–3.75×.
- [ ] **B7 USN Journal ile artımlı yeniden tarama (Windows)** — gecelik tarayan
      bir ajan için her seferinde her şeyi taramak israf. Stratejik olarak en
      büyük hız kazancı. *Rakip:* **SpaceObServer** — en yakın mimari rakibimiz
      ve tam bu noktada önde.

### C. Özellik açıkları

- [ ] **C1 E-posta uyarısı (hub)** — şu an yalnızca webhook.
      *Rakip:* SpaceObServer.
- [ ] **C2 Kişi başına hesap (hub)** — şu an tek admin kimliği.
      *Rakip:* SpaceObServer (Client/Web Access).
- [ ] **C3 Zaman çizelgesi görünümü (masaüstü)** — bir hedefin tüm geçmişi.
      Geçmiş ana iddiamız ama masaüstünde görselleştirilmiyor; **iddiamızın
      karşılığı olan görünüm eksik**, bu yüzden özellik listesinde önce geliyor.
- [ ] **C4 Duplicate bulucu** — boyut → ön-hash → blake3, önbellekli.
      WHY.md'de Pro kademesinde zaten planlı.
      *Rakip:* DiskRaptor (xxh3), WinDirStat 2.5.0, Czkawka.
- [ ] **C5 Tarama sırasında canlı büyüyen ağaç (masaüstü)** — FreeSize'ın manşet
      özelliği ve algılanan hızının büyük kısmı. Bizde ilerleme göstergesi var,
      canlı ağaç yok. Not: FreeSize bunu DOM'a çizerek yavaşlığının sebebi
      yaptı; Canvas2D + Rust yerleşimiyle aynı şeyi **hızlı** yapma avantajımız
      var.
- [ ] **C6 Sunburst görünümü (masaüstü)** — yalnızca treemap'imiz var.
      *Rakip:* FreeSize (treemap + sunburst + heatmap), Filelight.
- [ ] **C7 Dosya yaşı ısı haritası** — "2 yıldır dokunulmamış 400 GB". `mtime`
      zaten `Node`'da duruyor, yani ucuz. *Rakip:* FreeSize (heatmap).
- [ ] **C8 ncdu/gdu JSON içe aktarma** — dışa aktarabiliyoruz, içe alamıyoruz.
      Mevcut kullanıcıların eski taramaları bir edinim kanalı. *Rakip:* ncdu.
- [ ] **C9 CSV dışa aktarma** — kurumsal kullanıcının Excel'e attığı format.
      *Rakip:* WizTree.

### D. Sağlamlık

- [ ] **D1 Yavaş/yanıt vermeyen mount'ta timeout ve devam** — 1 TB USB HDD veya
      kopmuş NFS'te tarama takılırsa ne oluyor? Denenmedi.
      *Rakip:* DiskRaptor bu hatayı canlı yaşadı (issue #46: "1TB USB HDD'de
      30 saniye ilerleme yok") ve timeout/retry ekledi.
- [ ] **D2 Ajanda yerleşik TLS** — şu an ters vekil öneriliyor (Faz 2'de de var).
- [ ] **D3 Ajanda hız sınırlama** (Faz 2'de de var).
- [ ] **D4 10M+ dosyada bellek profili** — kısmen ölçüldü (412k girdide 122 MB,
      10M'e ekstrapolasyon ~2,8 GB). B1 sonrası yeniden ölçülmeli.
- [ ] **D5 `store::save` ilerleme geri bildirimi** — büyük ağaçlarda tek
      transaction, kullanıcı donmuş sanıyor.
- [ ] **D6 `Tree::rel_path` her çağrıda kökten yürüyor** — sıcak döngüde
      kullanılmamalı; ya belgelensin ya önbelleklensin.

### E. Dağıtım ve belge

- [x] **E1 `docs/RESEARCH.md`'den COMPETITORS.md'ye bağlantı** ve §1 rakip
      tablosunun ölçümlerle güncellenmesi. *(9 Eylül 2026)* dua-cli ayrı satıra
      çıktı; TreeSize Free MFT ve WinDirStat 2.5.0 düzeltmeleri işlendi;
      §3'teki ~25 B/dosya hedefinin yanına ölçülen 276–437 B ve 16 thread
      gerilemesi yazıldı. Yanlışlanan (b) maddesi silinmedi, **yanlışlandığı
      belirtilerek** bırakıldı — hangi kararın hangi bilgiyle verildiği kaybolmasın.
- [x] **E2 WHY.md düzeltmesi** — *"Uzak makine + tarama geçmişi yalnızca
      SpaceObServer'da var ve $600+/yıl; altında hiçbir şey yok"* cümlesi
      **artık yanlış**: FreeSize Pro CHF 29/yıl aynı vaadi veriyor, dua-cli
      diff'i ücretsiz. Fiyat hipotezi de bu cümleye dayanıyordu.
      *(9 Eylül 2026)* Beş yer düzeltildi: karşılaştırma tablosuna dua-cli sütunu
      ve **"self-host edilebilir" satırı** eklendi (kalın satır artık "geçmiş"
      değil, bu); "koca bir boşluk" paragrafı **üçlü kesişime** daraltıldı;
      "FreeSize'ın kopyalaması için sunucu yazması gerekir" cümlesi düzeltildi
      (yazdılar); fiyat çıpalarına CHF 29 eklendi; hız kazanma koşuluna ölçüm
      eklendi. **Ek:** README'deki *"**Every** disk analyser … FreeSize"* iddiası
      da yanlıştı, o da düzeltildi.
- [ ] **E3 Kod imzalama** — macOS Developer ID + notarization ($99/yıl),
      Windows OV ($150–300/yıl). *Rakip:* FreeSize, Diskaroo, TreeSize, WizTree
      — hepsi imzalı. Diskin her yerini okuyan imzasız bir program =
      SmartScreen/Gatekeeper uyarısı = düşük kurulum oranı.
- [ ] **E4 Homebrew / AUR / Microsoft Store.**
- [ ] **E5 Geçiş rehberleri** — "ncdu'dan geçiş", "TreeSize'dan geçiş"
      (ROADMAP'te 1.0 zorunlusu).

### Sıra

| Sıra | Ne | Neden burada |
|------|-----|--------------|
| ~~1~~ ✅ | ~~E1, E2~~ | Yarım saat, ve diğer her kararın girdisi — yanlış rekabet haritası üstüne plan yapılmasın. **Bitti (9 Eylül 2026)**, README düzeltmesi de dâhil |
| ~~2~~ ✅ | ~~A4, A1, A2, A4w~~ | Doğruluk iddiamız Windows'ta karşılanmıyordu. A4 (Unix) önce yapıldı çünkü test hiç yoktu. **Kod bitti (9 Eylül 2026), Windows CI onayı bekliyor** |
| 3 | B1 | Rakip 10 gün önce çözüp nasıl yaptığını yazdı; 10M dosya hedefinin önündeki duvar |
| 4 | A3, A5 | macOS'ta yanlışız (DaisyDisk doğru); ağ üzerinden bozulma sessiz |
| 5 | B2, B3 | Ucuz ve ölçülmüş — B2 eldeki veriyle hemen yapılabilir |
| 6 | C3, D1 | Geçmiş iddiamızın masaüstü karşılığı + hiç denenmemiş hata senaryosu |
| 7 | B4, B5, B6 | Platforma özel hızlı yollar — doğruluk düzeldikten **sonra** |
| 8 | B7 | Stratejik en büyük kazanç, ama en büyük iş |
| 9 | C1, C2, C4–C9 | Özellik paritesi |
| 10 | E3–E5, D2, D3 | Yayın hazırlığı |
| **son** | **B1-K** | Çift depolama. Sıradan optimizasyon değil, mimari karar — **ayrı oturumda Fable modeliyle derin araştırma**, önce benchmark altyapısı |

---

## Teknik borç

Ürünü bloke etmiyor ama biriktirmemeli. Yıldızlı olanlar artık "Rekabet
açıkları" bölümünde rakip bağlamı ve sırasıyla birlikte izleniyor — iş tanımı
orada, burada yalnızca borç kaydı olarak duruyorlar.

- [ ] **Windows `alloc` gerçek değil** — `GetFileInformationByHandleEx`
      (FILE_STANDARD_INFO) gerekiyor; şu an mantıksal boyuta eşit
      (`TODO(win)`, `crates/scan-core/src/meta.rs`) → **A1**
- [ ] **Windows hardlink dedupe kapalı** — `FileIdInfo` ile `(volume, file id)`
      → **A2**
- [ ] APFS clone tekilleştirme yok — macOS'ta `alloc` şişebilir → **A3**
- [ ] btrfs/ZFS: reflink ve sıkıştırma yüzünden ağaç yürüyüşü yanlış;
      "dosya sistemi farkında mod" gerekiyor → **A6**
- [x] ~~10M+ dosyalı köklerde bellek profili ölçülmedi~~ → **ölçüldü**
      (9 Eylül 2026): `Node` 104 B, gerçek tepe **276–437 B/girdi**
      (412k girdi = 122 MB RSS). Hedef ~25 B/dosya, yani **11–17 kat üstünde**;
      10M dosyaya ekstrapole ~2,8 GB. Düzeltme işi → **B1**, yeniden ölçüm → **D4**
- [ ] `Tree::rel_path` her çağrıda kökten yürüyor — sıcak döngüde kullanılmamalı
      → **D6**
- ~~Arayüz dizeleri koda gömülü, i18n yok~~ → borç değil, karar
      ([DECISIONS.md](docs/DECISIONS.md) K1). Dizeler İngilizce ve gömülü kalır.
- [ ] Büyük ağaçlarda `store::save` tek transaction — ilerleme geri bildirimi yok
      → **D5**

---

## Yayın sonrası — SEO ve AISEO

Kendi alan adı ve site deposu **9 Eylül 2026'da yapıldı**; kalanlar hâlâ
ertelenmiş durumda. Sıra geldiğinde audit'i baştan yapmak gerekmesin diye
ölçümler burada.

Durum tespiti (9 Eylül 2026, `dist` üzerinde ölçüldü) — **altyapı doğru**: 31
sayfa build sırasında HTML'e dönüyor, 26'sı hiç framework JS'i çekmiyor, React
yalnızca treemap demosunun olduğu 5 ana sayfada iniyor. Bu kritik, çünkü AI
tarayıcılarının çoğu (GPTBot, ClaudeBot, PerplexityBot) JavaScript
çalıştırmıyor; SPA olsaydı boş sayfa görürlerdi. Sayfa başına tek `<h1>`,
düzgün `h1→h2→h3`, `canonical`, beş dil için `hreflang` + `x-default`, 30
URL'lik sitemap ve Open Graph başlıkları hazır.

- [x] **Kendi alan adı** → `spacetrace.teknobakkall.com`. Cloudflare'de CNAME,
      **proxy kapalı** (turuncu bulutla GitHub sertifika üretemiyor).
      `base` `/` oldu, `public/CNAME` alan adını taşıyor.
- [x] **Site kendi deposuna alındı** →
      [unalcakir28/spacetrace-website](https://github.com/unalcakir28/spacetrace-website).
      Alan adıyla aynı geçişte yapıldı, çünkü depo adı URL'in parçasıydı.
- [ ] **`robots.txt` ve `llms.txt`** — alan adı geldiği için ikisi de artık
      mümkün. Site deposunda `public/` içine konur. `llms.txt`, AI ajanlarına
      projeyi tanıtan yerleşen konvansiyon.
- [ ] **JSON-LD yapısal veri** (şu an 0 tane). `SoftwareApplication` şeması "bu
      nedir, hangi işletim sistemi, hangi lisans, ücretsiz mi" sorularının
      makine tarafından okunabilir cevabı. Sayfa ve dil başına üretilmeli.
      Not: astro.build'in kendi sitesinde de yok, yani evrensel bir pratik
      değil — `og:image`'dan önce koymak fazla iddialı olur.
- [ ] **`og:image` ve Twitter kartı** (ikisi de yok). Şu an her paylaşım
      LinkedIn/X/Slack/Discord'da çıplak link olarak görünüyor. 1200×630 bir
      görsel gerekiyor; treemap'in kendisi doğal aday. astro.build'de var.
- [ ] **Google Search Console + Bing Webmaster Tools kaydı ve sitemap
      bildirimi.** "En hızlı indexlenme"nin gerçek cevabı bu ve kod işi değil,
      hesap işi. Bing ayrıca ChatGPT aramasını besliyor.
- [ ] Küçük: `og:locale` OG şartnamesinin istediği `en_US` biçiminde değil
      (`en` yazıyor); Google Fonts harici stylesheet olarak çekiliyor,
      woff2'leri kendimiz sunmak bir render-blocking üçüncü taraf isteğini
      kaldırır.

Beklenti ayarı: teknik taraf **indexlenmeyi engelleyen bir şey olmamasını**
sağlar ve içeriği maksimum okunur yapar. Sıralamada yukarı çıkmak içerik ve dış
bağlantı işi; yeni bir siteyi hiçbir teknik düzenleme hızla üst sıralara
taşımaz.

---

## Fikir havuzu

Karar verilmedi, sırası gelince tartışılacak.

- Duplicate bulucu (boyut → ön-hash → blake3, önbellekli)
- ncdu/gdu JSON **içe** aktarma (mevcut kullanıcıların eski taramaları)
- Cushion gölgelendirmeli treemap (SequoiaView/WinDirStat görünümü)
- Bulut kökleri: S3, OneDrive, Google Drive birer "uzak kaynak" olarak
- Paket yöneticisi farkındalığı (QDirStat'taki gibi: "bu dosya şu pakete ait")
- Dosya yaşı ısı haritası ("2 yıldır dokunulmamış 400 GB")
- `spacetrace watch` — inotify/FSEvents ile canlı güncelleme
- Prometheus metrik ucu (ajan `/metrics`)
