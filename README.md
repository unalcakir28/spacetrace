# spacetrace

Disk kullanımını tara, anlık görüntüsünü sakla, **neyin büyüdüğünü** gör.

Piyasadaki disk analizörleri (TreeSize, WizTree, DaisyDisk, FreeSize, ncdu) tek bir
soruyu cevaplıyor: *"şu an önümdeki diskte ne var?"*. spacetrace ikinci soruyu da
cevaplıyor: *"geçen haftadan beri ne değişti, ve hangi makinede?"*

Aynı ikili masaüstünde, sunucuda, NAS'ta ve konteynerde çalışır.

## Durum

| Faz | Kapsam | Durum |
|-----|--------|-------|
| 1 | Tarayıcı, SQLite anlık görüntü, diff, CLI | ✅ çalışıyor |
| 2 | Ajan (`serve` / `push`), uzak kaynaklar, Docker imajı | ⏳ sıradaki iş |
| 3 | Tauri masaüstü: treemap, uzak kaynak gezgini, diff görünümü | ⏳ |
| 4 | Merkez servis: çoklu makine panosu, büyüme uyarıları | ⏳ |

## Belgeler

| Belge | İçerik |
|-------|--------|
| [docs/WHY.md](docs/WHY.md) | Neden bu proje var: çıkış noktası, pazar boşluğu, hedef kullanıcı, kapsam dışı olanlar |
| [docs/ROADMAP.md](docs/ROADMAP.md) | Fazlar, çıkış kriterleri, sürüm hedefleri |
| [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) | Kodun nasıl ve neden böyle kurulduğu, teknoloji kararları, bilinen sınırlar |
| [TODO.md](TODO.md) | Canlı yapılacaklar listesi ve açık kararlar |

## Kurulum

Rust 1.85+ gerekir ([rustup.rs](https://rustup.rs)):

```bash
git clone <repo> && cd spacetrace
cargo build --release
./target/release/spacetrace --help
```

Tek ikiliyi PATH'e almak için: `cargo install --path crates/cli`

## Kullanım

```bash
# Tara ve özet gör
spacetrace scan ~/projects

# Tara ve anlık görüntü olarak kaydet
spacetrace scan /srv --save --label "haftalık"

# Kayıtlı anlık görüntüler
spacetrace scans

# Son iki kaydı karşılaştır: ne büyüdü?
spacetrace diff --path /srv

# Son kayıt ile diskin şu anki hâlini karşılaştır
spacetrace diff --since-last /srv

# Klasörleri boyuta göre listele
spacetrace ls /var/lib --top 20
spacetrace ls --scan 3 --subpath docker/overlay2

# ncdu ile aç (sunucudan aldığın kaydı yerelde incele)
spacetrace export --scan 3 --out scan.json && ncdu -f scan.json
```

Örnek çıktı:

```
#1 2026-09-06 17:23 [haftalık]  →  #2 2026-09-06 19:40
toplam 23.8 MiB → 76.3 MiB   (+52.5 MiB)

     DEĞİŞİM  DURUM         YENİ  YOL
   +40.1 MiB  büyüdü    42.9 MiB  app/logs/
   +14.3 MiB  büyüdü    25.7 MiB  backups/
    -1.9 MiB  silindi         0 B  uploads/
```

Dikkat: rapor `app/` veya `/srv` değil, **`app/logs/`** diyor. Değişimi yalnızca
aktaran ara klasörler atlanır; suçun gerçekten dağıldığı ilk seviye raporlanır.

### Sık kullanılan seçenekler

| Seçenek | Ne yapar |
|---------|----------|
| `--exclude node_modules` | O adı taşıyan klasörlere hiç girme (tekrarlanabilir) |
| `-x`, `--one-file-system` | Bağlama noktalarını aşma (`du -x` gibi) |
| `--depth N` | N seviyeden aşağı inme |
| `--min 10M` | diff'te bundan küçük değişiklikleri yok say |
| `--files` | diff'te dosyaları da raporla |
| `--json` | Çıktıyı JSON ver (her komutta) |
| `--db yol.sqlite` | Farklı anlık görüntü veritabanı |

Varsayılan veritabanı: macOS'ta `~/Library/Application Support/spacetrace/`,
Linux'ta `$XDG_DATA_HOME/spacetrace/`. `SPACETRACE_HOME` ile değiştirilebilir.

## Mimari

```
crates/
├── scan-core/   Paralel tarayıcı + arena ağaç modeli (platforma özel arka uçlar)
├── store/       SQLite anlık görüntü deposu + ncdu uyumlu dışa aktarım
├── diff/        İki anlık görüntüyü karşılaştırma, "suçlu klasör" tespiti
└── cli/         spacetrace ikilisi
```

Ağaç, çocukları bitişik indeks aralığında tutan bir **arena** olarak saklanır:
düğüm başına `Vec` yok, toplama tek ters geçişte biter, treemap yerleşimi için
önbellek dostu. Bu düzen SQLite'a olduğu gibi yazılır, yani bir anlık görüntüyü
yüklemek tek sıralı sorgudur, ağaç yeniden kurulmaz.

Sonraki fazlarda masaüstü uygulaması ve ajan **aynı çekirdeği** kullanacak; ajan
`scan-core` + `store` ile tek statik ikili olarak paketlenir.

### Boyut anlambilimi

- **mantıksal (`size`)**: yalnızca dosya baytları. `du -sb` ile birebir aynı.
- **diskte (`alloc`)**: gerçekten tahsis edilen bloklar, dizin blokları dâhil.
  `du -s --block-size=1` ile birebir aynı.
- Sabit bağlantılar (hardlink) varsayılan olarak bir kez sayılır; `--no-dedupe`
  ile kapatılır. Sembolik bağlantılar asla izlenmez, kendi boyutlarıyla sayılır.

Doğrulama: `/usr` (141k dosya), `/usr/share`, `/etc` üzerinde her iki toplam da
`du` ile **tam olarak** eşleşiyor.

## Geliştirme

```bash
cargo test --workspace     # 32 test
cargo clippy --workspace --all-targets
cargo fmt --all
```

Testler geçici dizinlerde gerçek dosya sistemi kullanır: sabit bağlantı,
sembolik bağlantı, izin hatası ve derinlik senaryoları dâhil.

## Lisans

Apache-2.0
