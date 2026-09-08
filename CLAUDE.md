# spacetrace — Claude için proje notları

Disk kullanımını tarayan, SQLite anlık görüntüsüne yazan ve iki görüntüyü
karşılaştırarak **neyin büyüdüğünü** söyleyen bir araç. Rust workspace.

Bağlam okuması (kodda görünmeyen kararlar): [docs/WHY.md](docs/WHY.md) neden bu
ürün, [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) neden bu tasarım,
[docs/DECISIONS.md](docs/DECISIONS.md) kapanmış fazlar arası kararlar ve
gerekçeleri, [docs/ROADMAP.md](docs/ROADMAP.md) fazlar, [TODO.md](TODO.md)
sıradaki iş. Bir özellik önerisini değerlendirirken WHY.md'deki **kapsam dışı**
listesine bak; bir tasarım kararını yeniden açmadan önce DECISIONS.md'ye bak.

## Komutlar

```bash
cargo test --workspace                   # 169 test, hepsi geçmeli
cargo clippy --workspace --all-targets   # uyarısız olmalı
cargo fmt --all
cargo build --release                    # ikili: target/release/spacetrace
cargo check -p spacetrace-scan-core --target x86_64-pc-windows-msvc
```

Windows tip denetimi yalnızca `scan-core` için yapılabiliyor: `agent` ve `cli`
zstd üzerinden C koduna bağlı ve macOS'ta msvc hedefi için çapraz derleyici yok.
Onların Windows davranışı ancak CI'da görülür — **CI'ı beklemeden "Windows'ta
çalışıyor" deme.**

Rust 1.85+ gerekir. Testler geçici dizinlerde **gerçek dosya sistemi** kullanır
(hardlink, symlink, izin hatası senaryoları dâhil), mock yok.

## Bozulmaması gereken değişmezler

Bunlar sessizce bozulabilir ve testler dışında fark edilmez:

0. **Bir veritabanını açmak yazma kilidi almamalı.** `store::schema::migrate`
   şema güncelse hiçbir DDL veya kalıcı pragma çalıştırmıyor. Ajan ve hub her
   istekte bağlantı açtığı için, koşulsuz `CREATE TABLE IF NOT EXISTS` ya da
   `PRAGMA journal_mode` bir okumanın süren yazmayı SQLITE_BUSY ile devirmesine
   yol açıyordu (CI'da yakalandı, testi `roundtrip.rs` içinde).
1. **Boyut anlambilimi.** `size` = yalnızca dosya baytları (`du -sb` ile birebir).
   `alloc` = tahsis edilen bloklar, dizin blokları dâhil (`du -s --block-size=1`
   ile birebir). Dizinlerin kendi inode boyutu mantıksal toplama **girmez**.
   Bu eşleşme bir test koşulu; `du` ile karşılaştırma testi eklemeden tarama
   davranışı değiştirme.
2. **Arena düzeni.** Düğümler BFS sırasında; bir düğümün çocukları bitişik
   (`children_start .. +children_len`) ve her çocuğun indeksi ebeveyninden
   **büyük**. `TreeBuilder::aggregate` tek ters geçişte topluyor ve `store`
   düzeni olduğu gibi saklıyor — sıra bozulursa ikisi de sessizce yanlış sonuç
   verir.
3. **Sembolik bağlantılar izlenmez** (kendi boyutlarıyla sayılır), **sabit
   bağlantılar bir kez sayılır** (`(dev, ino)`; ikinci kopya ağaçta görünür ama
   0 bayt katkı yapar).
4. **`Tree::remove_subtree` düğümü sıfırlar, listeden çıkarmaz.** Arena
   düzeninin anlamı budur: ortadan bir girdi kesmek sonrasındaki her düğümü
   yeniden numaralandırır ve elinde kimlik tutan her istemciyi (masaüstü) her
   şeyi unutmaya zorlar. Girdi adreslenebilir kalır ve 0 bayt bildirir;
   `children_len = 0` yapıldığı için altına inilemez. Dönen kimlik listesi
   çağıranın artık listelememesi gereken girdilerdir.
5. **İptal edilen tarama ağaç döndürmez.** `ScanProgress::cancel` sonrası
   `scan()` `ErrorKind::Interrupted` verir. Kısmi bir ağaç tam görünür ve
   yanlış toplam bildirir; onu gerçek snapshot'ların yanına yazmak en kötü
   sonuçtur.
6. **Hangi ölçüyle sıralandığı/çizildiği bir parametre, varsayılan değil.**
   `SizeBasis` (`Logical` | `OnDisk`) `children_by`, `Node::measure` ve
   `LayoutOptions.basis` üzerinden geçer. Seyrek bir dosya tuttuğundan 50 kat
   büyük bir uzunluk bildirir (1 TiB iddia eden Docker.raw 19 GiB tutuyor) ve
   bunlar gerçek disklerdeki *en büyük* girdiler — yani mantıksal ölçü en çok
   önemli olan girdilerde en çok yanılıyor. Sıralama ile yanındaki rakamın
   aynı ölçüden gelmesi zorunlu; "en büyük önce" diyen bir liste yanındaki
   sayıyla aynı şeyi söylemek durumunda. Masaüstü varsayılanı `OnDisk`, CLI
   `Logical` (çağrı yerlerinde açıkça yazılı).
7. **Hatalar yutulmaz.** Okunamayan yol sayılır ve örneklenir; tarama durmaz.
8. **Ajan hiçbir şeyi silmez.** Sunucuya kurulacak yazılımın güven kazanması için
   verilmiş bilinçli bir karar, eksik özellik değil.

## Kod ve depo alışkanlıkları

- **Her şey İngilizce: kod yorumları, kullanıcıya görünen dizeler, `--help`
  metinleri, hata mesajları.** i18n katmanı yok ve planlanmıyor (bkz.
  [docs/DECISIONS.md](docs/DECISIONS.md) K1). Türkçe kalan tek yer: WHY,
  ROADMAP, TODO, RESEARCH, DECISIONS ve bu dosya — bunlar gerekçe belgeleri.
- Yorum *ne yaptığını* değil **neden öyle yaptığını** anlatır. Kodun kendisi ne
  yaptığını zaten söylüyor.
- Bağımlılık eklemekte cimri ol. Ajanın tek statik ikili olarak NAS'a
  kurulabilmesi gerekiyor; bir bağımlılık eklemeden önce standart kütüphaneyle
  çözülüp çözülmediğine bak. Örnek: cron ayrıştırıcısı ve takvim aritmetiği
  chrono yerine elle yazıldı (~200 satır), çünkü tek ihtiyaç "bir sonraki eşleşen
  dakika"ydı. axum + tokio bilinçli bir istisna (bkz. DECISIONS K3).
- Yeni bağımlılıklar `[workspace.dependencies]` içinde sürümlenir, crate'ler
  `foo.workspace = true` ile alır.
- Commit mesajları Türkçe, gövde **neden** yapıldığını anlatır. Örnek için
  `git log` bak.
- `cargo clippy` uyarısı bırakma; CI `-D warnings` ile çalışıyor.

## Crate'ler

| Crate | Sorumluluk |
|-------|------------|
| `scan-core` | Tarama, ağaç modeli, platforma özel metadata. Hiçbir şeye bağlı değil. |
| `store` | SQLite anlık görüntü deposu, ncdu uyumlu dışa aktarım |
| `diff` | İki görüntüyü karşılaştırma, "suçlu klasör" tespiti |
| `cli` | `spacetrace` ikilisi (uzak kaynaklar dâhil) |
| `agent` | `spacetrace-agent` ikilisi: zamanlayıcı + HTTP servisi |
| `treemap` | Squarified yerleşim + LOD + hiyerarşik hit-test (masaüstü kullanır) |

Bağımlılık yönü tek yönlü. Ajan (Faz 2) bu üçünü kullanır, `cli`'ye
bağlanmaz.

## Depolar

Dört fazın hepsi çalışıyor. K2 gereği kod üç depoda:

| Depo | İçerik | Görünürlük |
|------|--------|------------|
| bu depo | scan-core, store, diff, treemap, cli, agent | public, Apache-2.0 |
| [spacetrace-desktop](https://github.com/unalcakir28/spacetrace-desktop) | Tauri v2 + React masaüstü | private, ticari |
| [spacetrace-hub](https://github.com/unalcakir28/spacetrace-hub) | Filo panosu, trend, uyarılar | private, ticari |

Diğer ikisi bu depoyu **git bağımlılığı** olarak kullanıyor, path değil. Yani
buradaki genel API'yi bozan bir değişiklik onları sessizce kırar; `main`'e
push etmeden önce bunu düşün.

## Sürüm ve site

Tam anlatım [docs/RELEASING.md](docs/RELEASING.md); kolay bozulan kısımlar:

- **Üç bileşenin indirilebilir dosyaları da bu deponun release'lerinde.** Private
  bir deponun release varlıkları kimlik doğrulaması olmadan indirilemiyor, o
  yüzden masaüstü ve hub kendi CI'larında derleyip buraya yayınlıyor
  (`RELEASE_TOKEN` sırrı ile). Etiketler: `continuous` / `v*` (CLI),
  `desktop-continuous` / `desktop-v*`, `hub-continuous` / `hub-v*`.
- **Etiket ve varlık adları sabit sözleşme.** `website/download.html` bunlara
  doğrudan bağlanıyor ve `install.sh` dosya adını verilen sürümden kuruyor;
  yeniden adlandırmak indirme sayfasını sessizce kırar.
- **Konteyner imajları önceden derlenmiş musl ikililerinden kuruluyor**
  (`.github/docker/Dockerfile.release`), kökteki `Dockerfile`'dan değil. QEMU
  altında Rust derlemek arm64 imajını dakikalar yerine on dakikalar sürdürüyor.
  Kökteki Dockerfile duruyor çünkü `docker build .` bir klonda çalışsın diye var.
- **`website/` yapı adımı olmayan düz HTML** ve JavaScript kapalıyken de çalışan
  bir indirme sayfası bırakmak zorunda: bağlantılar işaretlemede gerçek dosyalara
  işaret ediyor, `site.js` yalnızca sürüm/tarih/boyut ekliyor. Renk paleti
  masaüstünün `theme.css`'inden alınmış — ikinci bir palet icat etmek, ürüne
  benzemeyen bir site demek.
- Sürüm iş akışı belge ve site değişikliklerinde çalışmıyor (`paths-ignore`),
  Pages iş akışı yalnızca `website/**` değişince çalışıyor.

## Sıradaki iş

Kalan işler gerçek donanım veya gerçek zaman gerektiriyor (tam liste TODO.md):
gerçek sunuculara ajan kurulumu ve bir haftalık veri, Windows MFT hızlı yolu,
macOS Full Disk Access onboarding, WebKitGTK'da treemap performansı.

Fazlar arası kararlar kapandı ([docs/DECISIONS.md](docs/DECISIONS.md)); yeniden
açmadan önce oradaki gerekçeyi oku.

Kodda dikkat edilecekler:

- Snapshot telde **ham SQLite**. `Store::export_snapshot` ATTACH ile tek taramayı
  ayrı dosyaya kopyalar; `import_snapshot` kimliği yeniden atar ama host/root/
  `started_at` üçlüsünü korur — yinelenme kontrolü bu üçlüye dayanıyor.
- **`store::load` bir güven sınırı.** Uzaktan indirilen snapshot da bu yoldan
  geçiyor, bu yüzden `Tree::from_parts_checked` arena değişmezlerini doğruluyor.
  Doğrulamayı atlayan bir yol ekleme: bozuk `children_start` indeks panic'i,
  geriye dönük bir çocuk işaretçisi sonsuz döngü demek.
- Bir kök aynı anda yalnızca bir kez taranır (`Runner::try_claim`, HTTP'de 409).
- Zamanlayıcı UTC + sabit offset ile çalışır; saat dilimi veritabanı yok.
- Kapasite **boş/toplam** olarak raporlanır, "% dolu" olarak değil (K6).
  Modül dışına `capacity_of` adıyla açılıyor (`capacity::of` değil).
- **`ScanProgress` yalnızca sayaç değil, iptal anahtarı da.** Yürüyüş dizin
  başına bir kez kontrol ediyor — girdi başına kontrol en sıcak döngüye paylaşımlı
  bir atomik okuma koyardı ve okunmuş bir dizini bırakmak hiçbir şey kazandırmaz.

## Bilinen eksikler

Bunlara denk gelirsen bug değil, bilinen borç (tam liste TODO.md'de):

- Windows'ta `alloc` mantıksal boyuta eşit ve hardlink dedupe kapalı
  (`TODO(win)`, `crates/scan-core/src/meta.rs`) — gerçek değerler için
  `GetFileInformationByHandleEx` ve `FileIdInfo` gerekiyor.
- APFS clone'ları tekilleştirilmiyor; btrfs/ZFS'te reflink ve sıkıştırma
  yüzünden ağaç yürüyüşü gerçek kullanımı yanlış raporluyor.
- Tarama tüm ağacı bellekte tutuyor; 10M+ dosyada bellek profili ölçülmedi.
- Ajanda yerleşik TLS yok; ters vekil öneriliyor. Hız sınırlama da yok (token
  zaten gerekli olduğu için ertelendi).

## Bu depo dışındaki bağlam

Proje Cowork'te (claude.ai) başladı; oradaki oturum hafızası Claude Code'a
aktarılmaz, iki sistem ayrıdır. Aktarılması gereken her şey bu depodaki
belgelere yazıldı. Eylül 2026 pazar ve teknik araştırmasının özeti
[docs/RESEARCH.md](docs/RESEARCH.md) içinde.
