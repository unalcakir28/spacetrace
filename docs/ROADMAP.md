# Yol haritası

Fazlar sırayla ilerler ve her fazın bir **çıkış kriteri** vardır: o karşılanmadan
sonraki faza geçilmez. Sıralama ilkesi, ürünü farklılaştıran katmanın
(ajan + geçmiş + diff) görsel cilalardan **önce** gelmesidir; treemap "olması
gereken" bir özelliktir, ürünün kendisi değil.

Güncel yapılacaklar listesi için [../TODO.md](../TODO.md) dosyasına bakın.

---

## Faz 1 — Çekirdek ve CLI ✅

**Durum:** tamamlandı (Eylül 2026) · **Sürüm:** 0.1.0

Tek başına kullanılabilir bir komut satırı aracı. Bu faz aynı zamanda ajanın da
temelini kurar: ajan, bu çekirdeğin ağ arayüzü giydirilmiş hâli olacak.

- `scan-core`: paralel dizin taraması, arena ağaç modeli, hardlink
  tekilleştirme, exclude / one-filesystem / max-depth
- `store`: SQLite anlık görüntü deposu, ncdu uyumlu dışa aktarım, prune
- `diff`: iki anlık görüntüyü karşılaştırma, "suçlu klasör" tespiti
- `cli`: scan / ls / scans / diff / export / prune / rm

**Çıkış kriteri (karşılandı):** Toplamlar `du` ile birebir eşleşiyor; testler
üç platformda derleniyor; bir anlık görüntü kaydedilip bir hafta sonra
karşılaştırılabiliyor.

---

## Faz 2 — Ajan ⏳ sırada

**Hedef sürüm:** 0.2.0

Ürünün farklılaştığı ilk nokta. Aynı ikili sunucuda, NAS'ta ve konteynerde
çalışır; tarar, saklar, isteyene verir.

- `agent scan` — tek seferlik tarama, snapshot dosyasına yaz
- `agent serve` — HTTP+JSON servisi (axum, bkz. [DECISIONS.md](DECISIONS.md) K3):
  snapshot listesi, snapshot indirme, tetiklenen tarama; bearer token ile kimlik
  doğrulama, opsiyonel TLS
- `agent push` — snapshot'ı bir merkeze veya başka bir ajana gönder
- Ajanın kendi zamanlayıcısı (cron ifadesi) — NAS'ta systemd/cron kurcalamamak için
- Dağıtım: statik ikili (musl), Docker imajı, systemd unit dosyası,
  `curl | sh` kurulum betiği
- CLI'dan uzak kaynak okuma: `spacetrace scans --remote https://...`

**Çıkış kriteri:** Kendi Hetzner sunucularında, Proxmox host'unda ve bir Docker
konteynerinde ajan kurulu; her gece tarama alıyor; bir hafta sonra
`spacetrace diff --remote <host> --path /var` anlamlı çıktı veriyor.

**Bilinçli sınır:** Ajan hiçbir şeyi silmez, yalnızca okur. Bu ilk sürümde bir
özellik eksiği değil, güven kararı.

---

## Faz 3 — Masaüstü uygulaması ⏳

**Hedef sürüm:** 0.3.0 · **Yığın:** Tauri v2 + React/TS

- Klasör ağacı + zoom'lanabilir squarified treemap
- Dosya türüne göre renklendirme, çöpe gönder, Finder/Explorer'da aç
- "Uzak kaynak ekle": ajan URL'i veya SSH; uzak snapshot'ı yerelmiş gibi gezme
- Diff görünümü: büyüyen/küçülen klasörler, zaman çizelgesi
- Windows MFT hızlı yolu ("yönetici olarak çalıştır" arkasında), macOS Full Disk
  Access onboarding ekranı

**Çıkış kriteri:** 100k+ dikdörtgenlik bir treemap üç platformda akıcı
(pan/zoom'da kare düşürmüyor); uzak bir ajanın snapshot'ı yerel diskle aynı
arayüzde açılıyor.

---

## Faz 4 — Merkez servis ⏳

**Hedef sürüm:** 0.4.0

Ekip kademesinin karşılığı. Self-host edilebilir, zorunlu değil.

- Çoklu ajan panosu: hangi makinede ne kadar yer kaldı, ne büyüyor
- Klasör başına büyüme trendi ve eşik uyarıları (e-posta / webhook)
- Ekip erişimi, token yönetimi
- Web görünümü (masaüstündeki aynı React treemap bileşeni)

**Çıkış kriteri:** Beş makineyi izleyen bir kurulum, disk dolmadan önce
"şu klasör bu hızla giderse 9 gün sonra diski doldurur" uyarısı üretiyor.

---

## Faz 5 — Derinlik ⏳

Sıra fazla değil, talebe göre seçilir.

- Duplicate bulucu (boyut → ön-hash → blake3, önbellekli)
- Dosya sistemi farkındalığı: btrfs/ZFS snapshot ve reflink muhasebesi,
  APFS clone'ları, sıkıştırılmış kullanım
- Bulut kökleri: S3, OneDrive, Google Drive birer "uzak kaynak" olarak
- Cushion gölgelendirmeli treemap
- USN journal ile artımlı yeniden tarama (Windows)
- ncdu/gdu JSON içe aktarma (mevcut kullanıcıların eski taramaları)

---

## Sürüm hedefleri

| Sürüm | İçerik | Dağıtım |
|-------|--------|---------|
| 0.1 | CLI (Faz 1) | GitHub Releases, Homebrew tap, AUR |
| 0.2 | + ajan (Faz 2) | + Docker imajı, kurulum betiği |
| 0.3 | + masaüstü (Faz 3) | + imzalı .dmg / .msi / AppImage |
| 0.4 | + merkez (Faz 4) | + docker-compose |
| 1.0 | Faz 1–4 kararlı, İngilizce arayüz, belgelenmiş | Microsoft Store, Homebrew cask |

## Lansman öncesi zorunlular

Sürüm 1.0'dan önce, faz sırasından bağımsız olarak:

- **Arayüz dili İngilizce.** Hedef kanallar (HN, r/selfhosted, r/homelab,
  r/DataHoarder) İngilizce. Türkçe i18n ile geri gelebilir.
- **Kod imzalama.** macOS: Developer ID + notarization ($99/yıl). Windows: OV
  sertifika ($150–300/yıl) — Azure Artifact Signing Türkiye'den bireysel olarak
  alınamıyor, bu yüzden OV yolu planlanmalı.
- **Belgeler:** kurulum, ajan güvenlik modeli, "ncdu'dan geçiş" ve
  "TreeSize'dan geçiş" rehberleri.
