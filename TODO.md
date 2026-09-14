# Yapılacaklar

Canlı çalışma listesi. Faz tanımları ve çıkış kriterleri için
[docs/ROADMAP.md](docs/ROADMAP.md), gerekçeler için [docs/WHY.md](docs/WHY.md),
rakiplerin nerede önde olduğu için [docs/COMPETITORS.md](docs/COMPETITORS.md).

Son güncelleme: 14 Eylül 2026 (**Sürüm kesildi: CLI 0.7.0, masaüstü 0.7.0,
hub 0.5.0.** Şema v3'te sabit kaldığı için sıra zorunluluğu yoktu. Ayrıca
**B1-K bitti** — çift depolama kalktı, ölçüm
altyapısı depoda. Öncesi: **Sıra 1–4 kapandı** — E1, E2, A4, A1, A2,
A4w, B1, A3, A5. Üç platformda CI yeşil. 10 Eylül ayrıca
yayın günüydü: masaüstü 0.4.0 → 0.4.2, sonra A5 ile birlikte masaüstü 0.5.0,
hub 0.4.0 ve CLI 0.5.0. Şema atlaması olduğu için sıra zorunluydu: önce
masaüstü ve hub, sonra CLI (gerekçe docs/RELEASING.md). Ayrıca macOS FDA
onboarding ve **kararlı imza kimliği** girdi — E3'ün ücretsiz yarısı,
ayrıntısı aşağıda. Sıradaki iş → Sıra 5: **B2**, sonra B3)

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
- [x] `scan-core`: paralel DFS, arena ağaç (bitişik çocuklar, çocuk indeksi
      ebeveyninden büyük — başlangıçta BFS düzenindeydi, B1-K'den beri değil)
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
  - [ ] Opsiyonel yerleşik TLS — şimdilik ters vekil öneriliyor (→ **D2**)
- [x] `agent push <url>` — snapshot'ı merkeze/başka ajana gönder (zstd)
- [x] Eşzamanlı tarama kilidi (aynı kök iki kez taranmıyor → 409)
- [x] Hız sınırlama — **D3 yapıldı** (14 Eylül 2026); ertelenme gerekçesi
      yanlıştı, ayrıntısı orada

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
- [ ] **Windows MFT hızlı yolu** (`usn-journal-rs`) — yönetici arkasında (→ **B4**)
- [x] macOS Full Disk Access onboarding ekranı — karşılama ekranında bant,
      ayar paneline düğme, `Info.plist`'te altı kullanım açıklaması
- [x] Zaman çizelgesi görünümü (bir hedefin tüm geçmişi) — **C3**, masaüstü
      0.6.0'da yayınlandı
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
- [x] E-posta ile uyarı — **C1**, hub 0.5.0'da yayınlandı (`mailto:` hedefi,
      `[smtp]` config bölümü)
- [x] Kişi başına hesap — **C2**, hub 0.5.0'da yayınlandı (viewer/admin)

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
- [x] **A3 APFS clone tekilleştirme** — yapıldı *(9 Eylül 2026)*, varsayılan
      açık, `--no-clone-dedupe` ile kapanır (ajanda `dedupe_clones`).
      Clone'un kendi inode'u var ve `nlink == 1`, yani hardlink tekilleştirme
      onu göremiyor; ama diskte blokları bir kez duruyor. Kontrollü ölçüm:
      **3 clone × 100 MB = 0 MB** boş alan tüketimi, `du` ise 400 MB diyor.
      Tespit `fcntl(F_LOG2PHYS_EXT)` ile: aynı fiziksel offset'te başlayan
      dosyalar extent paylaşıyor. Sıkıştırılmış dosyalarda `ENOTSUP` dönüyor,
      o da güvenli biçimde "clone değil" demek. Yalnızca **boyutu başka bir
      dosyayla çakışan** dosyalar sorgulanıyor, çünkü her sorgu bir `open` +
      `fcntl`.

      **Ölçüm düzeltmesi — kendi rakamımı düzeltiyorum.** İlk ölçümüm
      "%31,7 fazla sayıyoruz" dedi ve **yanlıştı**: ölçüm betiğim hardlink'leri
      tekilleştirmiyordu, oysa tarayıcı `~/github`'da 68 bin hardlink eliyor.
      İki hardlink aynı inode'u paylaştığı için doğal olarak aynı fiziksel
      offset'i bildiriyor — yani "clone" saydıklarımın çoğu zaten hallettiğimiz
      hardlink'lermiş. Hardlink tekilleştirmesi sonrası **gerçek rakam:
      0,76 GiB / 15,6 GiB = %4,9**, 430 dosya. Bağımsız Python aracı ve
      tarayıcı birebir aynı sayıyı veriyor (430).

      | Ağaç | Kurtarılan | Maliyet |
      |------|-----------|---------|
      | `/Applications` (412k girdi) | 0 (hiç clone yok) | +91 ms (+8%) |
      | `~/github` (138k dosya) | 0,76 GiB (%4,9) | +72 ms (+21%) |

      **Değişmez #1 yeniden yazıldı:** `alloc` artık "`du` ile birebir" değil,
      "diskin gerçekten tuttuğu". Paylaşılan blok yokken `du` ile birebir aynı
      (test zorluyor), varken fark **tam olarak paylaşılan bloklar** (o da test
      ediliyor).
      *Rakip:* **DaisyDisk 4.34** aynı şeyi yapıyordu; artık biz de yapıyoruz.
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
- [x] **A5 Snapshot bütünlük kontrolü** — yapıldı *(10 Eylül 2026)*. Şema v3'te
      `scans.content_hash`: taramanın *mantıksal içeriğinin* SHA-256'sı
      (metadata satırı + her `entries` satırı, alanlar etiketli, dizeler
      uzunluk önekli). Dosyanın baytlarının değil — `export_snapshot` her
      seferinde yeni bir SQLite dosyası kuruyor ve `VACUUM` içeriği
      değiştirmeden dosyayı değiştiriyor; kendi kendine oynayan bir özet,
      olmayandan kötü. `import_snapshot` tutmazsa hiçbir şeyi almıyor,
      `export_snapshot` bozuk bildiğini göndermiyor, `spacetrace verify`
      istendiğinde bakıyor. `NULL` = "özet yok" (v3 öncesi), "bozuk" değil.
      **Kimlik doğrulaması değil** — gövdeyi değiştirebilen özeti de
      hesaplar; tehdit modeli bozulma, saldırgan değil.
      Yeni bağımlılık yok (`sha2` zaten workspace'te).
- [ ] **A6 btrfs/ZFS farkındalığı** — reflink ve sıkıştırma yüzünden ağaç
      yürüyüşü yanlış. Uzun vade; doğrusu örnekleme gerektiriyor.
      *Rakip:* btdu (Monte Carlo, ~100 örnekte %1 çözünürlük).

### B. Hız — ölçülmüş açıklar

- [x] **B1 Bellek** — *(9 Eylül 2026: ölçüldü, parçalandı, dördü yapıldı.
      Beşincisi ve asıl olanı, **B1-K**, 14 Eylül 2026'da bitti.)*

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
      - [x] **B1-K Çift depolamayı kaldır** — yapıldı *(14 Eylül 2026)*,
            aşağıdaki notta tarif edilen araştırma yapılıp **sorunun kendisi
            yanlış kurulmuş bulundu**.

      **Hedef düzeltmesi:** RESEARCH.md'deki **~25 B/dosya** hedefi bizim alan
      kümemizle **ulaşılabilir değil** ve karşılaştırma elmayla armut. ncdu 2
      düğüm başına `own_size`/`own_alloc`/`files`/`dirs` tutmuyor. Bizim
      taban aritmetiğimiz: en agresif daraltmayla `Node` 72 B + ad ~21 B =
      **~93 B/girdi**, artı çift depolama. Gerçekçi hedef **dua-cli'nin 64 B
      arena düğümü** mertebesi, 25 değil.
      *Rakip:* dua-cli 64 B arena düğümü + paylaşılan ad deposu, RSS %49 aşağı;
      ncdu 2 dosyada 25 B, dizinde 56 B.

      ### B1-K — çift depolama ✅ *(14 Eylül 2026)*

      **Cevap: ikilem yoktu.** Aşağıdaki dört soru araştırmanın çıkış
      noktasıydı ve birincisi ötekileri gereksiz kıldı.

      *"Çift depolama, değişmez #2 korunarak kaldırılabilir mi?"* — Evet, ve
      değişmez #2 sanıldığından zayıfmış. Arenanın istediği şey BFS değil, iki
      özellik: çocuklar bitişik, her çocuğun indeksi ebeveyninden büyük.
      Depodaki her tüketici tek tek denetlendi ve **hiçbiri seviye sırası
      istemiyor**: `aggregate` ve `median_bands` yalnızca ikinci özelliğe
      dayanan ters geçişler, `Tree::check` tam olarak o iki özelliği
      denetliyor, `store` düzeni olduğu gibi saklıyor, `diff` çocukları **ada
      göre** eşliyor, masaüstü id'leri generation'a bağlıyor. Kodda "BFS"
      yalnızca yorumlarda ve belgelerde geçiyordu.

      Bir dizinin listesi zaten tek thread'de tamamlandığı için, o an arenada
      bitişik bir blok açılıp yazılabiliyor (`TreeBuilder::push_block`).
      Ebeveyn adlandırılabilmek için arenada zaten olmak zorunda, yani ikinci
      özellik yapı gereği tutuyor. Ara ağaç tamamen kalktı.

      Seviye-senkron BFS'e, iş-çalan DFS'i riske atmaya, chunk'lı arenaya
      **gerek olmadı** — üçü de yanlış kurulmuş bir kısıtın çözümleriydi.
      dua-cli'nin yolu da okundu (`traverse.rs`): onlar da ara ağaç tutmuyor,
      girdileri akarken arenaya ekliyorlar; farkları tek tüketici thread +
      sınırlı kanal. Bize uymadı, çünkü listeleyen thread çocukların id'lerini
      recursion'dan **önce** bilmek zorunda.

      **Ölçüm** (`examples/memprobe.rs`, `scripts/bench-walk.sh`;
      serpiştirilmiş 9 tur, medyan, M3 Max):

      | | taban | sonra |
      |---|---|---|
      | `/Applications` tepe | 91,5 MB | **57,6 MB** (-%37) |
      | `/Applications` B/girdi, ipuçlu | 221 | **125** (-%43) |
      | `~/github` tepe | 234,6 MB | **201,6 MB** (-%14) |
      | Linux 75k, tepe | 15,0 MiB | **10,7 MiB** (-%29) |
      | Linux 75k, "ağaç olmayan" | 8,4 MiB | **4,1 MiB** (-%51) |

      Dört dağılımın hiçbiri örtüşmüyor. **Hız iki korpusta da aynı**:
      2022'ye karşı 1956 ms ve 739'a karşı 750 ms, ikisinde de aralıklar iç
      içe — yani söylenecek şey "aynı", iki medyanın farkı değil.

      **`~/github`'da kazancın küçük kalması B1-K'nin işi değil.** Orada
      tepenin ~140 MiB'ı canlı veri değil: taban ikili de ağacı düşürdükten
      sonra 201 MiB'da kalıyor. macOS libmalloc'un geri vermediği sayfalar,
      yani D4. Linux'ta aynı kod "ağaç olmayan" kısmı yarıya indiriyor ve
      yapısal sonuç o.

      **`ScanOptions::expected_entries`** eklendi: arena artık yürüyüş
      sırasında dolduğu için son boyutu baştan bilinmiyor, ve ikiye katlanan
      bir `Vec` son taşımada iki tamponu birden tutuyor (412k'da 57 MB; N =
      2^k+1'de arenanın iki katı). Bayrak değil — tek dürüst kaynağı aynı
      kökün önceki taraması. CLI ve ajan store'dan soruyor, masaüstü zaten
      ilerleme çubuğu için tuttuğu rakamı veriyor.

      **Gözlemlenebilir değişiklik:** iki tarama artık aynı id'leri vermiyor
      (aynı cevapları veriyor). `dupes`'un grup temsilcisi tarama içinde
      deterministik, iki tarama arasında yer değiştirebilir. Tarayıcının kendi
      clone tekilleştirmesi bu yüzden `(derinlik, yol)` ile sıralıyor.

      **Doğrulama:** `/Applications`'ın 412.983 satırı CSV dışa aktarımında
      birebir aynı. `/usr`'da ağacın şekli aynı, yalnızca hangi hardlink
      adının baytları taşıdığı değişiyor — değişmez 3 bunu zaten belirsiz
      ilan ediyor ve **eski ikili de kendi iki koşusu arasında 245 satırda
      değişiyor** (ölçüldü).

      **Hedef düzeltmesi hâlâ geçerli:** RESEARCH.md'deki ~25 B/dosya bizim
      alan kümemizle ulaşılabilir değil (ncdu 2 düğüm başına `own_size`/
      `own_alloc`/`files`/`dirs` tutmuyor). Taban aritmetiğimiz `Node` 72 B +
      ad ~21 B = ~93 B/girdi, ve ipuçlu ölçüm artık **125 B/girdi**, yani
      dua-cli mertebesinde.

- [x] **B2 Thread sayısı ayarı** — yapıldı *(10 Eylül 2026)*. `--threads N`,
      ajanda kök başına `threads`, ve yürüyüş artık rayon'un global havuzunda
      değil kendi havuzunda (kütüphane süreç geneli bir ayarı sahiplenemez).
      Varsayılan `min(çekirdek, 8)`. **En iyi thread sayısı diye bir şey yok:**
      optimum ağacın büyüklüğüyle kayıyor — 50k girdide 6, 412k'da 12. Seçilen
      sayı hiçbirinde en iyi değil ama ikisinde de eski varsayılanı yeniyor
      (%39 ve %11). Tablo ve yöntem docs/COMPETITORS.md §1.2.
      *Rakip:* erdtree ampirik 3 thread; TreeSize CPU yüküne göre ayarlıyor.
      **Açık kalan:** 412k üstü ölçülmedi. Sentetik 1.2M denemesi şekli
      bozuk çıktığı için atıldı; gerçek büyük bir korpus gerekiyor.
- [ ] **B3 HDD / ağ sürücüsü modu** — dönen diskte ve NFS'te paralel yürüyüş tek
      thread'den kötü olabilir (seek thrash); bu durum için hiçbir şeyimiz yok.
      *Rakip:* gdu `--sequential`; QDirStat girdileri stat etmeden önce inode'a
      göre sıralıyor.
- [ ] **B4 Windows MFT hızlı yolu** (`usn-journal-rs`) — yönetici gerekiyor,
      ReFS'te ve ağ/FAT'te yok → normal yola geri düşme şart, ve o yol A1/A2'de
      düzeliyor. **Sıra bu yüzden A'dan sonra.**
      *Rakip:* WizTree (ham MFT), TreeSize Free (yönetici), WinDirStat 2.5.0.
- [x] **B5 macOS `getattrlistbulk` hızlı yolu** — yapıldı *(11 Eylül 2026)*.
      Ayrı bir crate gerekmedi: `libc` zaten `getattrlistbulk`'ü açıyor.
      Dizin başına `readdir` + **girdi başına `lstat`** yerine tek çağrıda hem
      adlar hem metadata.

      **Ölçüm, serpiştirilmiş ve medyanlı, iki korpusta:**

      | Ağaç | Eski | Yeni | Kazanç |
      |------|------|------|--------|
      | `~/github` (297.695 girdi) | 1293 ms | 556 ms | **2,33×** |
      | `/Applications` (412k girdi) | 1554 ms | 646 ms | **2,41×** |

      Dağılımlar hiç örtüşmüyor (`~/github`: yeni maks 598, eski min 1225),
      yani bu makinenin gürültüsünün üretebileceği bir sayı değil. Yalnız
      listeleme katmanı tek thread'de ölçüldüğünde 3,3×; paralel yürüyüşte
      uçtan uca 2,3–2,4×, çünkü ağaç kurma ve clone sondası aynı kalıyor.
      Üç kökte (`~/github`, `/Applications`, `/usr`) iki ikilinin çıktısı
      birebir aynı.

      **Asıl risk ikinci bir kod yoluydu**, hız değil: sessizce ayrışan iki
      metadata kaynağı `store::digest`'in kendi başlığında uyardığı hata.
      O yüzden hızlı yol aynı `RawMeta`'yı üretiyor ve `assert_same_answer_as_lstat`
      iki yolu alan alan karşılaştırıyor — symlink, kırık symlink, dizine
      symlink, hardlink ve tek batch'e sığmayan dizin dâhil.

      **Dizin `nlink`'i düzeltilmek zorundaydı.** `ATTR_DIR_LINKCOUNT` APFS'te
      gerçek hardlink sayısını (1) veriyor, `st_nlink` ise her Unix aracının
      gösterdiği 2+altdizin'i. Dizin başına bir `lstat` ile eşitlendi;
      **ölçülen maliyeti yok** (3,31× vs 3,29×), çünkü çekirdek o inode'u az
      önce okumuş oluyor.

      **Mount içeren dizinde kullanılmıyor.** Dizinin tamamı tek çağrıda
      geliyor, yani cevap vermeyen bir dosya sistemine süre tanınacak girdi
      başına an kalmıyor — D1'in koruması eski yolu istiyor ve yürüyüş o
      dizinleri ona veriyor.

      **"Rakiplerin hiçbiri macOS'ta bunu yapmıyor" notu yanlıştı.**
      COMPETITORS.md §2'deki tablo DiskRaptor'ın `getattrlistbulk` kullandığını
      zaten yazıyordu, yani bu madde öne geçme değil eşitlenme. `dumac`'in
      `du`'ya karşı 6,39× rakamı da bizim 2,3×'imizle karşılaştırılamaz:
      onların tabanı tek thread'li `du`, bizimki zaten 8 thread'le paralel
      yürüyen kendi tarayıcımız.

      **İlk denemede `attribute_set_t`'yi bir slot kaydırmıştım** (`attrlist`
      ile karıştırıp; onun başlığı var, bunun yok) ve her girdi hatalı
      görünüyordu. Karşılaştırma fonksiyonum da sessizce geçiyordu, çünkü
      `zip` 9 ile 0'ı sıfır kez dönüyor — sayı eşitliği iddiası sonradan
      eklendi.
- [ ] **B6 Linux `getdents64` + `statx` hızlı yolu.**
      *Rakip:* `dut` sıcak cache'te `du`'dan 6.87×, dust/dua/gdu'dan 2.8–3.75×.
- [ ] **B7 USN Journal ile artımlı yeniden tarama (Windows)** — gecelik tarayan
      bir ajan için her seferinde her şeyi taramak israf. Stratejik olarak en
      büyük hız kazancı. *Rakip:* **SpaceObServer** — en yakın mimari rakibimiz
      ve tam bu noktada önde.

### C. Özellik açıkları

- [x] **C1 E-posta uyarısı (hub)** — yapıldı *(11 Eylül 2026)*. Bir kuralın
      hedefi ya http(s) URL'i ya da e-posta adresi; **tek sütun**, yanyana iki
      nullable alan değil, yani "tam olarak bir hedef" hatırlanması gereken bir
      kural değil verinin şekli. `mailto:` şeması seçildi çünkü v1'deki her
      webhook satırı zaten geçerli bir değer — göç dönüştürme değil, yeniden
      adlandırma (hub şeması v1 → v2, testi `db.rs` içinde, gerçekten v1'in
      yazdığı gibi kurulmuş bir veritabanıyla).
      **Kimlik bilgileri veritabanında değil**, config dosyasında
      (`password_file` ile, `admin_token` kalıbının aynısı): veritabanı
      yedeklenen ve hata raporuna eklenen dosya.
      **`lettre` bağımlılığı bilinçli** — cron ayrıştırıcısını elle yazdık ama
      SMTP öyle bir liste değil: TLS std'de yok, ve asıl ısıran kısım ağdan
      gelen bir hostname'in başlığa girmesi. Çıplak bir CRLF başlık bloğunu
      bitiriyor ve gerisi başlık olarak okunuyor. rustls-only alındı, `cargo
      tree`'de openssl/native-tls yok (musl statik derleme için şart).
      *Rakip:* SpaceObServer.
      - [ ] **Gerçek teslimat testi sizde.** Burada doğrulanan: rotalar (28
            entegrasyon testi, gerçek sokette), parolanın sayfaya sızmaması,
            CRLF'li alıcının reddi, v1→v2 göçü. **Doğrulanmayan: gerçek bir
            röleye gerçek posta.** Ayarlar → "Send a test message" tam olarak
            bunun için var ve bir uyarının izlediği yolun aynısını kullanıyor.
            Bakılacak: STARTTLS ile 587, implicit TLS ile 465, ve yanlış
            parolanın verdiği hatanın okunabilirliği.
- [x] **C2 Kişi başına hesap (hub)** — yapıldı *(11 Eylül 2026)*. Kişi başına
      token, iki rol: `viewer` her şeyi görür hiçbirini değiştiremez, `admin`
      erişimi kimin aldığı dâhil her şeyi değiştirir.
      **Parola değil token, ve bu bir kısayol değil sınır**: üretilmiş 256
      bitlik bir token düz SHA-256 olarak saklanabilir çünkü brute-force
      edilecek bir şey yok; insan seçimi bir parola saklanamazdı ve yavaş bir
      KDF ile etrafındaki her şeyi gerektirirdi. Giriş formu zaten token
      alıyordu, yani bu mekanizmaya ad ve rol eklemek oldu.
      **Yetkilendirme router'ın şekli, handler'daki bir kontrol değil.**
      Yazan rotalar kendi grubunda ve `require_write` katmanının arkasında;
      handler içindeki bir kontrol unutulabilir ve unutulması salt okunur bir
      hesabın filodaki her uyarı kuralını silebilmesi demek. İki guard var ve
      ikisinin de diş taşıdığı kanıtlandı: bir write rotasını okuma grubuna
      taşımak `a_viewer_can_read_everything_and_change_nothing`'i düşürüyor,
      listeye eklenmemiş yeni bir POST rotası `every_write_route_is_in_this_list`'i
      düşürüyor (o test `web.rs`'i okuyor, çünkü axum rotalarını dışarı
      vermiyor).
      Uyarı kuralları kimin eklediğini taşıyor (`created_by`, v2 satırlarında
      NULL — geriye doldurmak birinin adını yapmadığı bir işin üstüne yazmak
      olurdu). Son admin kendini kaldıramıyor: geri dönüş yolu sunucudaki
      dosya, ve web arayüzünden kendini kilitleyen birinin tam olarak
      bulunmadığı yer orası.
      *Rakip:* SpaceObServer (Client/Web Access).
- [x] **C3 Zaman çizelgesi görünümü (masaüstü)** — yapıldı *(11 Eylül 2026)*.
      Bir hedefin `(host, root)` geçmişi tek çizgide, aralarındaki değişim
      yanında, ve her adımdan tam o sıçramanın diff'ine bir tık.
      **Eksen sıfırdan başlıyor** — kendi verisine kırpılmış bir eksen %2'lik
      sürüklenmeyi uçuruma çeviriyor ve bu görünüm tam da "büyüyor mu?"
      sorusunu cevaplamak için var.
      **Gruplama Rust'ta** (`src-tauri/src/history.rs`), çünkü masaüstünde JS
      test koşucusu yok; kural içeren hiçbir şey test edilemeyen tarafta
      durmamalı (treemap yerleşimi de aynı sebeple Rust'ta). Kök yolu
      normalleştiriliyor: `/data` ile `/data/` bir hedefin geçmişini ikiye
      bölerdi.
      **Kapsam sınırı:** masaüstü *olanı* gösteriyor, hub *tahmin ediyor*.
      Hub'daki `trend.rs` en küçük kareler + `r2` eşiğiyle tahmin yapıyor ve
      kötü uyumda reddediyor; onun ikinci bir kopyasını masaüstüne koymak iki
      eşik kümesinin birbirinden ayrışması demekti. Taşımak istenirse `trend`
      önce çekirdek depoya alınmalı — ayrı iş.
- [x] **C4 Duplicate bulucu** — yapıldı *(11 Eylül 2026)*. `spacetrace dupes`,
      yeni `crates/dupes`. Üç kademe, her biri yalnızca bir öncekinin
      eleyemediğine ödeme yapıyor: boyut (bedava, tarama zaten biliyor) → ilk
      16 KiB → tam dosya. **Ölçüldü:** `~/github` (375.585 dosya, 33,8 GiB) →
      cevap 2,6 GiB okumaya mal oldu (%7,7); ikinci koşu 30 MiB okudu ve hiçbir
      şeyi yeniden özetlemedi.
      **BLAKE3, ve ardından bayt karşılaştırması yok** — 256 bit çıktıda
      çakışma olasılığı diskin yanlış bayt döndürmesinin çok altında, ve
      doğrulama geçişi bağlayıcı olmayan bir risk için okumayı ikiye katlardı.
      Bu, *kriptografik* bir özet hakkında bir iddia ve 64 bitlik biriyle
      savunulamazdı — xxh3 olmamasının sebebi bu.
      **Hardlink'ler duplicate değil**: baytlarını zaten paylaşıyorlar, ve
      geri kazanılabilir diye saymak silinince gelmeyecek yeri vaat etmek
      olurdu. Ayrı grupta ve sıfır kazançla listeleniyorlar. Dikkat: tarama
      hardlink'in ikinci adını 0 bayt yazıyor (değişmez 3), o yüzden boyut
      kademesi `nlink > 1` olanlar için diske bakıyor.
      **Önbellek snapshot veritabanında değil**, yanındaki ayrı dosyada
      (`<db>.hashes`): snapshot başka makineye taşınıyor ve yerel inode
      numaraları orada geçerli görünüp başka bir diske ait olurdu — ayrıca
      tablo eklemek `SCHEMA_VERSION`'ı ve onunla birlikte RELEASING.md'nin
      sürüm sırasını tetiklerdi, her an silinebilecek bir önbellek için.
      **Hiçbir şey silinmiyor**, ajanla aynı gerekçe: hangi kopyanın kalacağı
      bu crate'in sahip olmadığı bağlamı gerektiren bir karar.
      *Rakip:* DiskRaptor (xxh3), WinDirStat 2.5.0, Czkawka.
- [x] **C5 Tarama sırasında canlı büyüyen ağaç (masaüstü)** — yapıldı
      *(11 Eylül 2026)*. `scan-core/src/live.rs` + masaüstünde `LiveMap.tsx`.
      **Tek seviye, ve bu taslak değil karar**: treemap asgari alanın altını
      çizmiyor, yani tarama uçarken okunabilen tek kısım üst seviye, ve koşan
      bir taramaya sorulan soru her zaman "bunların hangisi büyük". Her
      seviyeyi taşımak, görülemeyecek kareler için diskle büyüyen ve her
      işçiden kilitlenen bir yapı demekti.
      **Baytlar dosya başına değil dizin başına ekleniyor** — en sıcak döngü
      girdi başına ve oraya paylaşımlı yazma koymamak için zaten ödeme
      yapılmıştı.
      **Maliyet ölçüldü:** yayın başına 4 ns (ilk hâli `RwLock` ile 203 ns'ti;
      "bir kez yazılır sonra okunur" zaten `OnceLock`'un anlamı). 34.000
      dizinlik `/Applications` için toplam 0,13 ms. Tüm taramanın A/B'si bunu
      çözemiyor (-%3,45 / -%0,45 — gürültü tabanı), o yüzden doğru form
      işlem başına ölçüm × işlem sayısı.
      **Aynı `squarify` rutini** hem canlı önizlemeyi hem bitmiş haritayı
      çiziyor (treemap crate'inde public edildi), yani tarama bitince resim
      kendini yeniden dizmiyor — devir teslim algoritma değişikliği değil veri
      değişikliği.
      **Yalnızca pencere boşken**: yeniden tarama okunmakta olan haritaya
      dokunmuyor, çünkü bu uygulamanın kuralı "iş pencereyi elinden almaz".
      Doğruluk iddiası ayrı testte: canlı toplamlar bitmiş ağacın toplamlarıyla
      birebir eşleşiyor (iki tamamen farklı yoldan gelen iki cevap).
      - [ ] **Görsel doğrulama sizde.** Ekran görüntüsü alamıyorum. Bakılacak:
            karelerin gerçekten büyüyüp yer değiştirdiği, etiketlerin dar
            karede taşmadığı, ve tarama bitip gerçek harita geldiğinde resmin
            zıplamadığı. Mantık tarafı test edildi (yerleşim, kap, ölçü,
            kategori, alan sınırları).
- [x] **C6 Sunburst görünümü (masaüstü)** — yapıldı *(11 Eylül 2026)*.
      `treemap/src/sunburst.rs` + masaüstünde `Sunburst.tsx`. Halka başına bir
      seviye, yayın açısı **ebeveynindeki pay** (kökteki değil) — halkaların
      alt alta hizalanmasının sebebi bu.
      **Açı boyut, alan değil.** Bir yayın alanı yarıçapla büyüyor, yani
      dışarıda duran küçük bir klasör içeride duran büyüğünden çok mürekkep
      kaplıyor. Görünümün sınırı, uygulamanın değil — treemap'in varsayılan
      kalmasının sebebi de bu, ve arayüz bunu ipucu metninde söylüyor.
      Ortadaki delik yalnızca etiket için değil: en içteki halkanın yayları
      aksi hâlde bir noktada birleşirdi ve en çok okunan seviye nişan alması
      en zor olan olurdu.
      Mutasyonla üç davranış doğrulandı, üçü de yakalandı: ebeveyn payı yerine
      kök payı, hit-test'in açı kuralı, çok ince yayların elenmesi.
      *Rakip:* FreeSize (treemap + sunburst + heatmap), Filelight.
      - [ ] **Görsel doğrulama sizde.** Bakılacak: halkaların hizalandığı,
            etiketlerin dar yayda taşmadığı, imlecin işaret ettiğini seçtiği
            (özellikle saat 12 dikişinde), ve dar/geniş pencerede dairenin
            elips olmadığı.
- [x] **C7 Dosya yaşı** — yapıldı *(11 Eylül 2026)*. `spacetrace age`
      (`--bands`, `--json`) ve masaüstünde yaşa göre renklendirilen harita.
      Hesap çekirdekte (`scan-core/src/age.rs`), C3'teki kalıpla: masaüstünde
      JS testi yok, kural içeren şey test edilebilir tarafta durur.
      **Bayta göre ağırlıklı, dosya sayısına göre değil** — yüz bin eski kaynak
      dosyası cevap değil, bir disk imajı cevap. **Dizinler profile girmiyor:**
      bir dizinin `mtime`'ı yanına bir şey eklenince değişiyor, içindekilerin
      yaşıyla ilgisi yok. **`mtime` yoksa ayrı bant** — ncdu'dan gelen snapshot
      onu taşımıyor ve 1970 okumak "elli yıldır dokunulmamış" demek olurdu.
      **Haritada dizin de renkleniyor** ve bandı alt ağacının *medyan baytı*
      (`median_bands`) — kendi `mtime`'ı değil. Isı haritasını yeniden
      renklendirilmiş kategori haritasından ayıran şey bu: bakılmaya değer her
      derinlikte alanın çoğu klasör, ve hareket edilebilen birim klasör.
      Medyan, "en çok bayt hangi bantta" yerine seçildi çünkü ikincisi 51/49
      bölünmüş bir klasörü tek bir renge boyuyor ve renk tek dosyada dönüyor.
      *Rakip:* FreeSize (heatmap).
      - [ ] **Görsel doğrulama sizde.** Bu makinede ekran görüntüsü izni yok,
            yani rampanın okunurluğu, lejandın dar pencerede taşıp taşmadığı ve
            klasör tonunun (%55 alfa) çocuklarını gömüp gömmediği
            **denenmedi**. Hesap tarafı doğrulandı: `/Applications`'ın en büyük
            300 dizininde `median_bands` ile alt ağaç profilinin medyanı
            bağımsız iki yürüyüşle karşılaştırıldı, sıfır uyuşmazlık.
- [x] **C8 ncdu/gdu JSON içe aktarma** — yapıldı *(11 Eylül 2026)*.
      `spacetrace import <dosya>`; `--root`, `--host`, `--label`. Yeni
      bağımlılık yok, `serde_json` zaten `store`'daydı.

      **Toplama ikinci kez yazılmadı.** İçe aktarıcı `Tree::from_nested`'e
      veriyor, o da yürüyüşün kullandığı `TreeBuilder` + `aggregate`'i
      çağırıyor. Arena değişmezleri ve toplama tek uygulamada kalıyor.

      **Mutasyon testi iki gerçek hata çıkardı.** Birincisi: `from_nested`
      `children_start`/`children_len` doldurmuyordu (`push` yapmıyor, tarayıcı
      onu `flatten`'da yapıyor) — ağacın toplamları doğru, her `children()`
      çağrısı boştu. Layout testi de bu yüzden **boş bir iddiaydı**:
      `children_len = 0` olunca iki döngü de hiç dönmüyordu. Fixture'da bir
      alt dizinden *sonra* kardeş yoktu, o yüzden DFS mutasyonu bile
      geçiyordu; fixture düzeltilince hata çıktı.

      İkincisi ve daha ciddisi: **gerçek ncdu dizinlere de `asize` yazıyor** ve
      ben onu mantıksal toplama ekliyordum — **değişmez 1 ihlali**, GNU
      `du --apparent-size`'ın bizden ayrıldığı noktanın ta kendisi. Kendi
      dışa aktarıcımız dizinlere `asize: 0` yazdığı için gidiş-dönüş testi
      bunu asla göremezdi; spec'ten yazılmış gerçekçi bir ncdu fixture'ı
      yakaladı.

      **Doğrulanmayan tek şey:** gerçek bir `ncdu` çıktısı. ncdu bu makinede
      kurulu değil (kurmak izin isterdi), fixture spec'ten yazıldı.
- [x] **C9 CSV dışa aktarma** — yapıldı *(11 Eylül 2026)*.
      `spacetrace export --format csv`, `--depth` ile üst katmanlarda durma.
      **İki ölçü de sütun, ayar değil** — dosyada sıralama ve yanındaki etiket
      olmadığı için değişmez 6'nın gerekçesi burada geçmiyor; okuyan seçsin.
      Alt ağaç toplamlarının yanında `own_*` sütunları var, yoksa bütün
      satırların toplamı bir dosyayı üstündeki her klasör için tekrar sayar.
      RFC 4180 kaçırma elle yazıldı ve testi bir CSV *okuyucusuyla* yapılıyor:
      altın dize karşılaştırması, tutarlı biçimde yanlış üreten bir hatayı da
      geçerdi.

### D. Sağlamlık

- [x] **D1 Yavaş/yanıt vermeyen mount'ta timeout ve devam** — yapıldı
      *(görünürlük 10, timeout 11 Eylül 2026)*.
      *Rakip:* DiskRaptor bu hatayı canlı yaşadı (issue #46: "1TB USB HDD'de
      30 saniye ilerleme yok") ve timeout/retry ekledi.

      **Sorun.** `read_dir_parallel` her girdi için `entry.metadata()`, yani
      bir `lstat` çağırıyor ve bu `dev` karşılaştırmasından **önce**. Ölü bir
      mount'ta o syscall dönmüyor ve taşınabilir şekilde kesilemiyor. Üç
      sonucu vardı: `--one-file-system` korumuyordu (kontrol, hang'e sebep
      olan `lstat`'ın verisini kullanıyor), bir girdi bulunduğu dizinin
      **tamamını** kilitliyordu (dizin okuma bilinçli olarak tek thread'te),
      ve iptal kurtarmıyordu (`is_cancelled()` yalnızca `read_dir`'den önce).

      **Çözüm.** Dokunmadan önce sınırı bilmek: `mounts.rs` tarama başında
      mount tablosunu okuyor (macOS `getmntinfo(MNT_NOWAIT)` — **`MNT_WAIT`
      değil**, o da ölü mount'ta bloke oluyor; Linux `/proc/self/mountinfo`,
      saf std; diğerleri boş küme = eski davranış). Mount noktasına
      terk edilmeye razı olunan bir thread üzerinden yaklaşılıyor
      (`timeout.rs`); cevap gelmezse **okunamayan yol** sayılıyor
      (değişmez 7) ve yürüyüş kardeşlerle devam ediyor. Kısmi ağaç
      döndürülmüyor (değişmez 5).

      **`libc` zaten oradaydı.** Bu maddenin engeli diye yazdığım "önce
      `scan-core`'a `libc` eklenmeli" yanlıştı: `capacity.rs` ve `meta.rs`
      zaten kullanıyor.

      **Varsayılan 60 sn, cömert bilerek.** İki hatanın maliyeti eşit değil:
      fazla beklemek taramayı yavaşlatır, erken vazgeçmek **bütün bir birimi**
      tam olduğunu iddia eden bir toplamdan sessizce düşürür. `--mount-timeout 0`
      eski davranışı geri veriyor; ajanda kök başına `mount_timeout`.

      **Maliyet: dizin başına bir hash sorgusu, ~95 ns.** `~/github` (297.695
      girdi, 10.856 dizin) için **1,03 ms**, yani ~%0,09. Girdi başına değil
      dizin başına, çünkü `Mounts` mount noktalarının *ebeveynlerini* de
      tutuyor: mount içermeyen bir dizin hiçbir girdisini sorgulatmıyor.
      **Bütün-tarama A/B'si bu işi yapamıyor** — +%9,19 gösterdi, yani gerçek
      maliyetin 100 katı; 1 ms, ±150 ms'lik koşu gürültüsünün içinde
      görünmüyor. Sayı mikro-benchmark'tan ve dizin sayımından geliyor.

      **Doğrulanamayan tek şey:** gerçek bir çekirdek seviyesi asılmanın bu
      yola girdiği. Sonda enjekte edilebilir olduğu için mekanizmanın
      tamamı test ediliyor (atlanıyor, kardeşler taranıyor, toplam şişmiyor,
      sağlıklı mount normal taranıyor, kapatınca sonda hiç çağrılmıyor) —
      ama asılan mount'u üretmek için ikinci bir makine gerekiyor.

- [ ] **D2 Ajanda yerleşik TLS** — şu an ters vekil öneriliyor (Faz 2'de de var).
- [x] **D3 Ajanda hız sınırlama** — yapıldı *(14 Eylül 2026)*.
      `crates/agent/src/ratelimit.rs`, elle yazılmış token bucket, yeni
      bağımlılık yok (cron ayrıştırıcısıyla aynı gerekçe).

      **Ertelenme gerekçesi yanlıştı.** "Token zaten gerekli" başka bir soruyu
      cevaplıyor: token *kimin* okuyabileceğine karar veriyor, *ne sıklıkla*
      okuyabileceğine değil. İki şey tokenin tamamen dışında: `/health`
      bilinçli olarak tokensiz, ve yanlış bir token da bir başlık ayrıştırma,
      bir karşılaştırma ve bir cevaba mal oluyor. Sınırlayıcı bu yüzden
      **auth'tan önce** çalışıyor — bunları sınırlayabileceği tek konum orası,
      ve mutasyonla doğrulandı (`/health`'i muaf tutunca test düşüyor).

      Pencere değil kova: pencere sınırında sınırın iki katı geçebiliyor
      (bir pencerenin son anı + sonrakinin ilk anı), ve "ani yoğunluk olur,
      sürekli akış olmaz" ancak kovayla ifade edilebiliyor.

      **Ters vekil uyarısı belgede yazılı.** Proje ters vekil öneriyor ve orada
      her istek vekilin adresinden geliyor, yani istemci başına sınır herkesin
      paylaştığı tek sınıra dönüşüyor. `X-Forwarded-For` **okunmuyor**:
      herkesin yazabildiği bir başlık, yazarak yeni bir hak almanın yolu olurdu.

      Tablo sert sınırlı (4096 adres). Dolduğunda ve herkes hâlâ borçluyken
      yeni adresler reddediliyor — akış hâlindeki birini unutmak ona dolu bir
      hak geri verir, yani tablo sınırın etrafından dolaşmanın yolu olurdu.
      Bunu kendi testim yakaladı: ilk hâlimde yalnızca "dolu" kovalar
      atılıyordu ve her yeni istemci hemen jetonunu harcadığı için hiçbiri dolu
      olmuyordu, yani tablo sınırsız büyüyordu.

      Varsayılan 120/dakika + 60 ani; `0` kapatıyor. Sekiz birim testi (saat
      testin elinde, yoksa her iddia bir `sleep` ve bir tahmin olurdu) ve üç
      entegrasyon testi gerçek sokette.
- [~] **D4 10M+ dosyada bellek profili** — **ölçüldü** *(11 Eylül 2026)*.
      Eskiden burada tek noktadan bir ekstrapolasyon vardı (~2,8 GB); gerçek
      cevap ondan hem daha iyi hem daha ilginç.

      **Ağacın kendisi: 96 bayt/girdi, 100k'dan 10M'e kusursuz doğrusal, ve
      macOS ile Linux'ta birebir aynı** (10M girdi = 915–916 MiB). Sentetik
      ağaç `TreeAssembler` ile kuruldu, yani `store::load`'un kullandığı yol —
      yapının modeli değil, yapının kendisi.

      **Ama RSS ağaç değil, ve aradaki fark platforma bağlı.** 250k girdilik
      aynı ağaçta:

      | | macOS | Linux (glibc) |
      |---|---|---|
      | tek tarama RSS | 125 MiB (~4× ağaç) | **35 MiB** (~1,2× ağaç) |
      | 12–20 tarama sonra | **941 MiB** | **37,9 MiB** |
      | tarama başına | +42 MiB | +0,2 MiB |

      **Aynı kod.** Linux her zaman `read_dir` + `lstat` yolunu kullanıyor ve
      düz; macOS'ta *aynı yol* (bulk kapatılarak ölçüldü) tarama başına
      +26 MiB büyüyor. Yani bu bir sızıntı değil, macOS libmalloc'un
      parçalanmış span'ları geri vermemesi. `malloc_zone_pressure_relief`
      denendi: **hiçbir şey yapmıyor** (RSS birebir aynı). `getattrlistbulk`
      yolu ayırma trafiğini artırdığı için büyümeyi +26'dan +45 MiB'a
      çıkarıyor — sebep değil, çarpan.

      Elenen açıklamalar: thread sayısı (1'de 138 MiB, 16'da 163 — gürültü),
      thread havuzu sızıntısı (3 girdilik ağaçta 20 tarama boyunca RSS sabit).

      *Sonuç:* **Ajan için sorun yok** — Linux'ta 10M girdi ≈ 1,1 GB ve
      tekrarlı taramalar birikmiyor. *Kalan:* **masaüstünde "Yeniden tara"**
      aynı süreçte tekrar tarıyor ve macOS'ta her seferinde büyüyor; uzun bir
      oturumda görünür. Ucuz bir çare yok (relief işe yaramıyor), gerçek çare
      yürüyüşteki ayırma trafiğini azaltmak — yani B1.

      **Düzeltme (14 Eylül 2026): "tarama başına +42 MiB" korpusa bağlıymış
      ve bu satır onu söylemiyordu.** B1-K'nin taban ölçümü aynı prob'u iki
      ağaçta koşturdu, 8 tur, tek süreçte:

      | | `/Applications` | `~/github` |
      |---|---|---|
      | girdi | 412.983 | 428.731 |
      | 1. tur | 87,4 MiB | 204,8 MiB |
      | 8. tur | 94,8 MiB | 445,0 MiB |
      | tarama başına | +1,0 MiB (düzleşiyor) | +34 MiB (düzleşmiyor) |

      Neredeyse aynı girdi sayısı, tamamen farklı davranış. Ayırt eden şey
      ayırma trafiği: `~/github`'ın adları 21,7 MiB, `/Applications`'ınki
      8,2 MiB. Yani "macOS'ta her tarama +42 MiB" bir üst sınır, kural değil.

      **B1-K sonrası (14 Eylül 2026):** ayırma trafiği düştüğü için tepe de
      düştü — `/Applications` 91,5 → 57,6 MB, Linux'ta "ağaç olmayan" kısım
      yarıya indi. Birikme macOS'ta duruyor (sebep libmalloc, kod değil) ama
      daha düşük bir tabandan başlıyor, ve masaüstü artık `expected_entries`
      ipucunu geçiyor.
- [x] **D5 `store::save` ilerleme geri bildirimi** — yapıldı
      *(14 Eylül 2026)*. **Ve önce yeniden ölçüldü, çünkü eski rakam yanlıştı.**
      11 Eylül'deki "en çok ~290 ms" dolaylı bir üst sınırdı (toplam süreden
      tarama çıkarılarak). Doğrudan A/B: `/Applications`'ta kaydetmeden 753 ms,
      kaydederek 1324 ms → **571 ms**, yani tahminin iki katı. 10M girdiye
      ≈ **14 saniye**. "Acil değil" değerlendirmesi bu rakamla ayakta durmuyor.

      İçi de ölçüldü: satır yazma **273 ms**, `digest` **208 ms**, commit
      18 ms. İki geçiş de aynı mertebede olduğu için iki ayrı faz:
      `Phase::Saving` ve `Phase::Checksumming`. Tek faz olsaydı çubuk sonuna
      varıp baştan başlardı.

      `ScanProgress`'e `rows_done`/`rows_total` eklendi ve `StallWatch`'ın
      sayaç dizisine girdi — o dizinin `ScanProgress`'ten kendi okuması tam
      olarak bunun için tasarlanmıştı, yani her izleyici değiştirilmeden
      kapsadı. CLI'ın ilerleme satırı artık RAII bir muhafız: eskiden yürüyüş
      biter bitmez temizleniyordu ve komut yarım saniye sessiz kalıyordu.
      Ajanın `/status`'u da kaydetme boyunca cevap veriyor (`rows_done`,
      `rows_total`, `phase: "saving"`).

      Testi kaydetmeyi **yan thread'den izliyor**: "doğru sayıda bitti" bir
      sayacın en sonda tek adımda zıplamasıyla da doğru olurdu, ve öyle bir
      sayaç bütün faz boyunca donmuş görünür. İki mutasyonla doğrulandı.
- [x] **D6 `Tree::rel_path` her çağrıda kökten yürüyor** — yapıldı
      *(14 Eylül 2026)*. Ne belgelendi ne önbelleklendi: **yönü çevrildi.**
      `Tree::for_each_path` yolu inerken kuruyor — bir dizine girmek tampona
      bir parça ekliyor, çıkmak onu kesiyor — yani her ad altındaki girdi
      sayısı kadar değil, bir kez yazılıyor. Önbellek düşünülmedi bile: bellek
      B1-K'de zorlukla kazanılmıştı.

      **Ölçüldü:** `/Applications`'ın 412.983 girdisinde `rel_path` ile
      **61 ms**, inerek **4,5 ms** — **13,6×**, beş turda dağılımlar hiç
      örtüşmüyor. Uçtan uca CSV dışa aktarımı 449 → **400 ms** (medyan,
      serpiştirilmiş 7 tur; OLD min 445 > NEW medyan). Kontrol olarak yol
      kurmayan ncdu dışa aktarımı 182 → 183 ms, yani değişiklik tam beklenen
      yerde.

      **`dupes`'ta ölçülebilir fark yok** (2970 → 2909 ms, dağılımlar
      örtüşüyor) çünkü komutun süresini tarama ve `stat` belirliyor. Oradaki
      gerekçe hız değil en kötü durum: `Tree::path` derinlikle doğrusal ve
      derinliği bu crate belirlemiyor — başka makineden gelen bir snapshot
      istediği kadar derin olabilir, yani her dosya için çağırmak kötü girdide
      karesel.

      **Yan bulgu ve gerçek bir hata:** `dupes` altı yerde düğüm id'siyle
      sıralıyordu ve her birinin yorumu "iki koşu aynı sonucu versin"
      diyordu — ama B1-K'den beri id'ler iki tarama arasında değişiyor.
      Yayınlanmış 0.7.0 sabit bir fikstürde her koşuda farklı sıra veriyor
      (ölçüldü). Altısı da yola çevrildi. Mevcut test bunu göremezdi: `find`'ı
      **tek bir ağaç** üzerinde beş kez çağırıyordu, yani id'ler zaten aynıydı.

      CSV satır sırası da değişti (artık katı DFS: klasör, hemen ardından
      içeriği) — changelog'da yazılı.

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
      **Yarısı 10 Eylül 2026'da ücretsiz kapandı:** Gatekeeper ile TCC ayrı
      sistemler ve ikincisi yalnızca *kararlı bir kimlik* istiyordu. Kendinden
      imzalı sertifika girdi, izinler artık güncellemeden sağ çıkıyor. Açık
      kalan yalnızca Gatekeeper/SmartScreen uyarısı, ve onun bedeli para.
      Geçici yama olarak `install-desktop.sh` / `.ps1` var — curl karantina
      damgası yazmadığı için uyarı hiç tetiklenmiyor. Ödeme yapıldığında
      silinecekler docs/RELEASING.md'de madde madde yazılı.
- [ ] **E4 Homebrew / AUR / Microsoft Store.**
- [x] ~~**E5 Geçiş rehberleri**~~ — **kapsam dışı** *(14 Eylül 2026, kullanıcı
      kararı)*. ROADMAP 1.0 zorunlusu diye listelemişti; öyle değil. Ürünün
      kendisi zaten ncdu çıktısını içeri alıyor (C8) ve `--help` ile README
      İngilizce, yani "nasıl geçerim" sorusunun cevabı araçta duruyor. Bir
      rehber yazmak mühendislik değil pazarlama, ve bu sıraya ait değil.

### Sıra

| Sıra | Ne | Neden burada |
|------|-----|--------------|
| ~~1~~ ✅ | ~~E1, E2~~ | Yarım saat, ve diğer her kararın girdisi — yanlış rekabet haritası üstüne plan yapılmasın. **Bitti (9 Eylül 2026)**, README düzeltmesi de dâhil |
| ~~2~~ ✅ | ~~A4, A1, A2, A4w~~ | Doğruluk iddiamız Windows'ta karşılanmıyordu. A4 (Unix) önce yapıldı çünkü test hiç yoktu. **Bitti (9 Eylül 2026), Windows CI yeşil** |
| ~~3~~ ✅ | ~~B1~~ | Rakip 10 gün önce çözüp nasıl yaptığını yazdı; 10M dosya hedefinin önündeki duvar. **Dördü bitti (9 Eylül 2026), B1-K 14 Eylül'de** |
| ~~4~~ ✅ | ~~A3, A5~~ | macOS'ta yanlıştık (DaisyDisk doğruydu) — A3 bitti; ağ üzerinden bozulma artık sessiz değil. **Bitti (10 Eylül 2026)** |
| 5 | ~~B2~~ ✅, **B3 ← sıradaki** | Ucuz ve ölçülmüş — B2 bitti (10 Eylül 2026); B3 gerçek bir HDD ya da ağ sürücüsü istiyor |
| ~~6~~ | ~~C3, D1~~ | İkisi de yapıldı |
| 7 | B4, ~~B5~~ ✅, B6 | Platforma özel hızlı yollar — B5 bitti (11 Eylül 2026); B4 Windows, B6 Linux makinesi istiyor |
| 8 | B7 | Stratejik en büyük kazanç, ama en büyük iş |
| 9 | ~~C1–C9~~ ✅ | Özellik paritesi tamamlandı *(11 Eylül 2026)* |
| 10 | E3, E4, D2, D3 | Yayın hazırlığı (E5 kapsam dışı bırakıldı) |
| ~~son~~ ✅ | ~~B1-K~~ | Çift depolama. Araştırma yapıldı ve **sorun yanlış kurulmuştu**: arena BFS istemiyormuş, iki özellik istiyormuş. Ara ağaç kalktı, hız aynı, tepe bellek `/Applications`'ta -%37. **Bitti (14 Eylül 2026)** |

---

## Teknik borç

Ürünü bloke etmiyor ama biriktirmemeli. Yıldızlı olanlar artık "Rekabet
açıkları" bölümünde rakip bağlamı ve sırasıyla birlikte izleniyor — iş tanımı
orada, burada yalnızca borç kaydı olarak duruyorlar.

- [x] ~~Windows `alloc` gerçek değil~~ → **A1 yapıldı** *(9 Eylül 2026)*.
      Bu satır 11 Eylül'e kadar açık duruyordu ve işaret ettiği `TODO(win)`
      işareti koddan çoktan kalkmıştı — yani borç listesi aynı dosyanın yol
      haritası bölümüyle çelişiyordu. Kapanmış bir borcu açık göstermek,
      birinin bitmiş işi yeniden yapması demek.
- [x] ~~Windows hardlink dedupe kapalı~~ → **A2 yapıldı** *(9 Eylül 2026)*.
- [x] ~~APFS clone tekilleştirme yok~~ → **A3 yapıldı** *(9 Eylül 2026)*,
      `fcntl(F_LOG2PHYS_EXT)` ile, varsayılan açık.
- [ ] btrfs/ZFS: reflink ve sıkıştırma yüzünden ağaç yürüyüşü yanlış;
      "dosya sistemi farkında mod" gerekiyor → **A6**
- [x] ~~10M+ dosyalı köklerde bellek profili ölçülmedi~~ → **ölçüldü**
      (9 Eylül 2026): `Node` 104 B, gerçek tepe **276–437 B/girdi**.
      Sonra düzeltildi: `Node` **72 B** (B1), çift depolama kalktı (B1-K), ve
      14 Eylül 2026'da tepe `/Applications`'ta **125 B/girdi** (ipuçlu).
      Ölçüm aracı artık depoda: `examples/memprobe.rs`. Kalan → **D4**
      (macOS libmalloc birikmesi, kod değil)
- [x] ~~`Tree::rel_path` her çağrıda kökten yürüyor~~ → **D6 yapıldı**
      (14 Eylül 2026): `Tree::for_each_path` yolu inerken kuruyor, 13,6× hızlı,
      ve `dupes`'un id'ye dayalı sıralaması yola çevrildi.
- ~~Arayüz dizeleri koda gömülü, i18n yok~~ → borç değil, karar
      ([DECISIONS.md](docs/DECISIONS.md) K1). Dizeler İngilizce ve gömülü kalır.
- [x] ~~Büyük ağaçlarda `store::save` tek transaction — ilerleme geri bildirimi
      yok~~ → **D5 yapıldı** (14 Eylül 2026). Tek transaction duruyor; eksik
      olan sayaçtı. Gerçek maliyet 571 ms/412k (eski "≤290 ms" tahmininin iki
      katı), 10M'e ≈ 14 s.

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
