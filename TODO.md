# Yapılacaklar

Canlı çalışma listesi. Faz tanımları ve çıkış kriterleri için
[docs/ROADMAP.md](docs/ROADMAP.md), gerekçeler için [docs/WHY.md](docs/WHY.md).

Son güncelleme: 9 Eylül 2026 (dört faz çalışıyor; site ve sürüm hattı yayında)

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

## Teknik borç

Ürünü bloke etmiyor ama biriktirmemeli.

- [ ] **Windows `alloc` gerçek değil** — `GetFileInformationByHandleEx`
      (FILE_STANDARD_INFO) gerekiyor; şu an mantıksal boyuta eşit
      (`TODO(win)`, `crates/scan-core/src/meta.rs`)
- [ ] **Windows hardlink dedupe kapalı** — `FileIdInfo` ile `(volume, file id)`
- [ ] APFS clone tekilleştirme yok — macOS'ta `alloc` şişebilir
- [ ] btrfs/ZFS: reflink ve sıkıştırma yüzünden ağaç yürüyüşü yanlış;
      "dosya sistemi farkında mod" gerekiyor
- [ ] 10M+ dosyalı köklerde bellek profili ölçülmedi (hedef: ncdu2 mertebesi,
      ~25 B/dosya)
- [ ] `Tree::rel_path` her çağrıda kökten yürüyor — sıcak döngüde kullanılmamalı
- ~~Arayüz dizeleri koda gömülü, i18n yok~~ → borç değil, karar
      ([DECISIONS.md](docs/DECISIONS.md) K1). Dizeler İngilizce ve gömülü kalır.
- [ ] Büyük ağaçlarda `store::save` tek transaction — ilerleme geri bildirimi yok

---

## Yayın sonrası — SEO ve AISEO

Karara bağlandı ama **bilinçli olarak ertelendi** (9 Eylül 2026): ürün yayına
çıkmadan bunlara girmek erken. Sıra geldiğinde audit'i baştan yapmak gerekmesin
diye ölçümler burada.

Durum tespiti (9 Eylül 2026, `website/dist` üzerinde ölçüldü) — **altyapı
doğru**: 31 sayfa build sırasında HTML'e dönüyor, 26'sı hiç framework JS'i
çekmiyor, React yalnızca treemap demosunun olduğu 5 ana sayfada iniyor. Bu
kritik, çünkü AI tarayıcılarının çoğu (GPTBot, ClaudeBot, PerplexityBot)
JavaScript çalıştırmıyor; SPA olsaydı boş sayfa görürlerdi. Sayfa başına tek
`<h1>`, düzgün `h1→h2→h3`, `canonical`, beş dil için `hreflang` + `x-default`,
30 URL'lik sitemap (dil alternatifleri `xhtml:link` ile içinde), `description`
ve Open Graph başlıkları hazır.

Eksikler, getirisi yüksek olandan başlayarak:

- [ ] **Kendi alan adı** (ör. `spacetrace.dev`). En büyük kalem, çünkü diğer
      ikisini o açıyor: `robots.txt` ve `llms.txt` yalnızca alan adının
      **kökünde** geçerli, `/spacetrace/robots.txt` tarayıcılar tarafından
      okunmaz — ve `https://unalcakir28.github.io/` şu an **404** (kullanıcı
      sitesi deposu yok), yani kök bizim değil. Ayrıca github.io'nun bir alt
      klasöründe olmak alan adı otoritesini paylaşmak demek.
      Gerekenler: `website/public/CNAME`, DNS kaydı, `astro.config.mjs` içinde
      `site` yeni alan adı ve `base: "/"`. Dikkat: `localeUrl()` iç linkleri
      kendiliğinden düzeltir ama **elle yazılmış** adresler düzelmez —
      `website/src/data/releases.ts`, üç README, `docs/RELEASING.md` ve
      masaüstü deposunun sürüm notundaki indirme linki.
- [ ] **JSON-LD yapısal veri** (şu an 0 tane). Hem SEO hem AISEO'nun en yüksek
      getirili kalemi: `SoftwareApplication` şeması "bu nedir, hangi işletim
      sistemi, hangi lisans, ücretsiz mi" sorularının makine tarafından
      okunabilir cevabı. Sayfa ve dil başına üretilmeli.
- [ ] **`og:image` ve Twitter kartı** (ikisi de yok). Şu an her paylaşım
      LinkedIn/X/Slack/Discord'da çıplak link olarak görünüyor. 1200×630 bir
      görsel gerekiyor; treemap'in kendisi doğal aday.
- [ ] **`llms.txt`** — AI ajanlarına projeyi tanıtan, yeni yerleşen
      konvansiyon. Alan adının kökünde durmak zorunda, yani 1. maddeyi bekler.
- [ ] **Google Search Console + Bing Webmaster Tools kaydı ve sitemap
      bildirimi.** "En hızlı indexlenme"nin gerçek cevabı bu ve kod işi değil,
      hesap işi. Bing ayrıca ChatGPT aramasını besliyor.
- [ ] Küçük: `og:locale` OG şartnamesinin istediği `en_US` biçiminde değil
      (`en` yazıyor); Google Fonts harici stylesheet olarak çekiliyor,
      woff2'leri kendimiz sunmak bir render-blocking üçüncü taraf isteğini
      kaldırır.

Beklenti ayarı: teknik taraf **indexlenmeyi engelleyen bir şey olmamasını**
sağlar ve içeriği maksimum okunur yapar. Sıralamada yukarı çıkmak içerik ve dış
bağlantı işi; yeni ve paylaşılan bir alan adındaki bir siteyi hiçbir teknik
düzenleme hızla üst sıralara taşımaz.

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
