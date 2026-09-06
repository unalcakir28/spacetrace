# Yapılacaklar

Canlı çalışma listesi. Faz tanımları ve çıkış kriterleri için
[docs/ROADMAP.md](docs/ROADMAP.md), gerekçeler için [docs/WHY.md](docs/WHY.md).

Son güncelleme: 7 Eylül 2026

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

## Faz 3 — Masaüstü ⏳

- [ ] Tauri v2 + React/TS iskeleti, Rust çekirdeğini süreç içinde çağırma
- [ ] Squarified treemap (layout Rust'ta, çizim Canvas2D → gerekirse WebGL)
- [ ] LOD: ~4–6 px²'den küçük dikdörtgenleri bölme; quadtree ile culling
      ve hit-test
- [ ] Klasör ağacı paneli, çift yönlü seçim senkronu
- [ ] Dosya türüne göre renklendirme, çöpe gönder, Finder/Explorer'da aç
- [ ] "Uzak kaynak ekle" akışı; uzak snapshot'ı yerelmiş gibi gezme
- [ ] Diff görünümü ve zaman çizelgesi
- [ ] **Windows MFT hızlı yolu** (`usn-journal-rs`) — yönetici arkasında
- [ ] macOS Full Disk Access onboarding ekranı
- [ ] Üç WebView'da treemap performans testi (WebKitGTK dâhil)

---

## Faz 4 — Merkez ⏳

- [ ] axum + Postgres/SQLite servisi, self-host
- [ ] Çoklu ajan panosu
- [ ] Klasör başına büyüme trendi, eşik uyarıları (e-posta / webhook)
- [ ] "Bu hızla giderse N gün sonra dolar" tahmini
- [ ] Ekip erişimi ve token yönetimi
- [ ] docker-compose ile tek komut kurulum

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
