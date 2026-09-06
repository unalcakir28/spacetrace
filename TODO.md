# Yapılacaklar

Canlı çalışma listesi. Faz tanımları ve çıkış kriterleri için
[docs/ROADMAP.md](docs/ROADMAP.md), gerekçeler için [docs/WHY.md](docs/WHY.md).

Son güncelleme: 6 Eylül 2026

---

## Açık kararlar

Kod yazmadan önce cevaplanması gerekenler.

- [ ] **Arayüz dili.** CLI ve belgeler şu an Türkçe. Hedef kanallar (HN,
      r/selfhosted) İngilizce; 1.0 öncesi geçiş şart. Şimdi mi, yoksa ajan
      bittikten sonra mı? Erken yapmak ucuz.
- [ ] **Lisans modeli.** Çekirdek + ajan açık kaynak (Apache-2.0), masaüstü
      ticari mi? Ajanın sunucuya kurulması için açık kaynak olması güven
      açısından güçlü bir argüman.
- [ ] **Ajan protokolü:** HTTP+JSON mı, gRPC mi? HTTP daha kolay hata ayıklanır
      ve curl ile test edilir; gRPC akış (streaming) ve şema disiplini verir.
- [ ] **Snapshot taşınabilirliği:** ajan SQLite dosyasını mı gönderecek, yoksa
      ara bir serileştirme formatı mı olacak? (SQLite dosyası basit ama büyük.)
- [ ] **GitHub deposu** açık mı özel mi başlasın?

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

## Faz 2 — Ajan ⏳ sıradaki iş

### Çekirdek
- [ ] `agent` crate'i (workspace'e ekle, `spacetrace-agent` ikilisi)
- [ ] `agent scan <yol> --out snap.sqlite` — tek seferlik, ağsız
- [ ] Yapılandırma dosyası (TOML): kökler, hariç tutulanlar, zamanlama, token
- [ ] Dahili zamanlayıcı (cron ifadesi) — NAS'ta systemd/cron kurcalamamak için
- [ ] Snapshot rotasyonu (ajan tarafında `prune`)

### Ağ
- [ ] `agent serve` — HTTP servisi
  - [ ] `GET /health`, `GET /scans`, `GET /scans/:id` (snapshot indir)
  - [ ] `POST /scans` (tarama tetikle)
  - [ ] Bearer token doğrulama; token dosyadan veya env'den
  - [ ] Opsiyonel TLS; ters vekil arkasında çalışabilme
- [ ] `agent push <url>` — snapshot'ı merkeze/başka ajana gönder
- [ ] Hız sınırlama ve eşzamanlı tarama kilidi (aynı kök iki kez taranmasın)

### İstemci tarafı
- [ ] CLI'da uzak kaynak: `--remote https://host` (scans / ls / diff)
- [ ] Uzak kaynak tanımlarını yerel yapılandırmada saklama (`~/.config/spacetrace/remotes.toml`)
- [ ] SSH modu: kurulum gerektirmeden karşı tarafta geçici ajan çalıştırma
      (TreeSize'ın SSH taramasının karşılığı)

### Dağıtım
- [ ] Statik ikili (musl) — linux/amd64, linux/arm64
- [ ] Docker imajı (volume'ları read-only mount ile tarar)
- [ ] systemd unit + örnek yapılandırma
- [ ] `curl | sh` kurulum betiği
- [ ] GitHub Releases otomasyonu (tag → build → artefakt)

### Doğrulama (çıkış kriteri)
- [ ] Kendi Hetzner sunucularına, Proxmox host'una ve bir konteynere kur
- [ ] Bir hafta gerçek veri topla
- [ ] `spacetrace diff --remote <host> --path /var` anlamlı çıktı veriyor mu?

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
- [ ] Arayüz dizeleri koda gömülü, i18n yok
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
