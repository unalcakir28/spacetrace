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
cargo test --workspace                   # 402 test, hepsi geçmeli
cargo clippy --workspace --all-targets   # uyarısız olmalı
cargo fmt --all
cargo build --release                    # ikili: target/release/spacetrace
cargo check -p spacetrace-scan-core --target x86_64-pc-windows-msvc --all-targets
cargo run -q -p spacetrace-changelog -- markdown --component cli > CHANGELOG.md
```

**`--all-targets` şart:** onsuz test kodu hiç derlenmiyor ve `#[cfg(test)]`
altındaki platforma bağlı bir hata ancak CI'da görünüyor (11 Eylül 2026'da
öyle oldu: `parse_mountinfo` testte her platformda derleniyordu ve içi
`std::os::unix` kullanıyordu).

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
1. **Boyut anlambilimi.** `size` = yalnızca dosya baytları. `alloc` = **diskin
   gerçekten tuttuğu**, dizin blokları dâhil. Dizinlerin kendi inode boyutu
   mantıksal toplama **girmez**; bu yüzden `size` `du -sb` ile eşleşmez (GNU
   `--apparent-size` her dizinin inode boyutunu ekliyor) — eski hâli öyle
   diyordu, yanlıştı.

   **`alloc` = `du` değil, `alloc` ≈ `df`.** Paylaşılan blok yokken ikisi
   birebir aynı ve test bunu zorluyor. Blok paylaşımı varken `du` fazla
   sayıyor, biz saymıyoruz, ve **fark tam olarak paylaşılan bloklar**
   (bu da test ediliyor). İki durum var: hardlink'te `du` da tekilleştiriyor,
   **APFS clone'unda tekilleştirmiyor** — clone'un kendi inode'u var ve
   `nlink == 1`, ama diskte blokları bir kez duruyor. Ölçüldü: 3 clone × 100 MB
   = **0 MB** boş alan tüketimi. Bunu `du`'ya uyarak raporlamak, "diskte ne
   kadar yer kaplıyor" sorusuna yanlış cevap vermek olurdu.
   Bu eşleşme bir test koşulu ve **testi
   `crates/scan-core/tests/du_equivalence.rs`** (9 Eylül 2026'da yazıldı; o güne
   kadar iddia elle doğrulanıyordu). `alloc` için oracle harici `du`; `size`
   için oracle aynı dosyadaki naif seri yürüyüş, çünkü `du` mantıksal boyutu
   veremiyor (BSD `-A` bloğa yuvarlıyor, GNU `--apparent-size` dizin inode'unu
   ekliyor). Tarama davranışını değiştirirken bu dosyayı genişlet.
2. **Arena düzeni: iki özellik, ve yalnızca iki.** Bir düğümün çocukları
   bitişik (`children_start .. +children_len`), ve her çocuğun indeksi
   ebeveynininkinden **büyük**. `TreeBuilder::aggregate` tek ters geçişte
   topluyor (ikinci özellik bunun için), `store` düzeni olduğu gibi saklıyor,
   `Tree::check` tam olarak bu ikisini denetliyor.

   **BFS değil, ve bu 14 Eylül 2026'da değişti.** Yıllarca "BFS sırasında"
   yazdı çünkü öyleydi: yürüyüş ayrı bir ara ağaç kuruyor, `flatten` onu
   seviye seviye kopyalıyordu. Artık her dizin listelenir listelenmez arenaya
   yazılıyor (B1-K, çift depolama kalktı), yani **düzen dizinlerin bitiş
   sırası**. İki özellik yapı gereği tutuyor — ebeveyn adlandırılabilmek için
   zaten arenada olmak zorunda. Çocuk eklemenin tek yolu
   `TreeBuilder::push_block`.

   **Sonuç: iki tarama aynı düzeni vermez.** Aynı diskin iki taraması aynı
   cevapları verir (test: `the_thread_count_does_not_change_the_answer`, yol
   yol karşılaştırıyor) ama aynı id'leri vermez. Zaten hiçbir tüketici
   vermesine güvenmiyordu — `diff` ada göre eşliyor, masaüstü id'leri
   generation'a bağlıyor — ama **id sırasına dayanan yeni bir şey yazma.**
   Fark eden tek yer `dupes`: grup temsilcisi en düşük id ve bu tarama içinde
   deterministik, ama iki tarama arasında "ilk kopya" yer değiştirebilir.
   Tarayıcının kendi clone tekilleştirmesi bu yüzden `(derinlik, yol)` ile
   sıralıyor, id ile değil.
3. **Sembolik bağlantılar izlenmez** (kendi boyutlarıyla sayılır), **sabit
   bağlantılar bir kez sayılır** (`(dev, ino)`; her iki ad da ağaçta görünür,
   biri 0 bayt katkı yapar). **Hangi adın baytları taşıdığı belirsizdir** —
   yürüyüş paralel, inode'u önce talep eden thread kazanıyor ve bu platformdan
   platforma değişiyor (macOS'ta kökteki kopya, Linux'ta içteki kopya sayıldı;
   CI'da yakalandı). Garanti "bir kez", "ilk yol" değil — test yazarken
   çifte iddia et, tek ada değil.
4. **`Tree::remove_subtree` düğümü sıfırlar, listeden çıkarmaz.** Arena
   düzeninin anlamı budur: ortadan bir girdi kesmek sonrasındaki her düğümü
   yeniden numaralandırır ve elinde kimlik tutan her istemciyi (masaüstü) her
   şeyi unutmaya zorlar. Girdi adreslenebilir kalır ve 0 bayt bildirir;
   `children_len = 0` yapıldığı için altına inilemez. Dönen kimlik listesi
   çağıranın artık listelememesi gereken girdilerdir.
5. **İptal edilen tarama ağaç döndürmez.** `ScanProgress::cancel` sonrası
   `scan()` `ErrorKind::Interrupted` verir. Kısmi bir ağaç tam görünür ve
   yanlış toplam bildirir; onu gerçek snapshot'ların yanına yazmak en kötü
   sonuçtur. **Testi yazarken:** "yürüyüş erkenden durdu" iddiası
   *tarama başlamadan* iptal edip `progress.files == 0` doğrulanarak kurulur.
   Yan thread'den iptal edip "hepsini bitirmemiş olmalı" demek, yürüyüşün
   zamanlayıcıdan yavaş olduğuna bahis oynamaktır ve hızlı makinede kaybeder
   (macOS CI'da kaybetti).
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
   **Cevap vermeyen bir mount da bir okuma hatasıdır.** `entry.metadata()` ölü
   bir mount'ta dönmüyor ve kesilemiyor, o yüzden mount noktalarına
   (`mounts.rs`, tarama başında okunur) terk edilebilir bir thread üzerinden
   yaklaşılıyor; süre dolunca yol okunamayan sayılıp yürüyüş kardeşlerle
   devam ediyor. Tabloyu `MNT_NOWAIT` ile oku — `MNT_WAIT` ölü mount'ta
   bloke oluyor, yani önlem bug'a dönüşüyor.
8. **Uzun süren her aşamanın kımıldayan bir sayacı olmalı.** İzleyen taraf
   "takıldı mı" sorusunu yalnızca sayaçlara bakarak cevaplıyor (CLI 10 saniye
   hareketsizlikte uyarıyor), yani sayacı olmayan bir aşama sağlıklı çalışırken
   asılmış görünür. Clone sondası tam bunu yapıyordu: `~/github`'da 1989 ms'lik
   taramanın 1193 ms'i, tek bir sayaç kımıldamadan (ölçüldü, 10 Eylül 2026).
   `clones_probed` bu yüzden var; yeni bir aşama eklerken aynısını yap.
   **Kaydetme de bir aşama** (14 Eylül 2026): 412.983 girdide yürüyüş 753 ms,
   veritabanına yazmak 571 ms — 10M girdiye ≈ 14 saniye. `Phase::Saving` ve
   `Phase::Checksumming` (iki ayrı geçiş: 273 ms yazma, 208 ms özet) ile
   `ScanProgress::rows_done`/`rows_total` bunun için var, ve CLI ilerleme
   satırı artık taramayla birlikte kaydetmeyi de kapsıyor. **`StallWatch`
   sayaçları `ScanProgress`'ten kendi okuyor**, tam da yeni bir sayaç
   eklendiğinde her izleyicinin onu değiştirilmeden görmesi için.
9. **Ajan hiçbir şeyi silmez.** Sunucuya kurulacak yazılımın güven kazanması için
   verilmiş bilinçli bir karar, eksik özellik değil.

## Kod ve depo alışkanlıkları

- **Bu depoda her şey İngilizce: kod yorumları, kullanıcıya görünen dizeler,
  `--help` metinleri, hata mesajları.** CLI, ajan ve hub için i18n katmanı yok
  ve planlanmıyor — çevrilmiş bir komut yanlış bilgidir (K1). Türkçe kalan tek
  yer: WHY, ROADMAP, TODO, RESEARCH, DECISIONS ve bu dosya — gerekçe belgeleri.
- **İstisna, ve yalnızca iki yerde: masaüstü GUI ve changelog metinleri beş
  dilde** (`en tr it fr de`, sitedekiyle aynı küme). Bu K1'in iptali değil,
  kapsamının daraltılması; gerekçe
  [docs/DECISIONS.md](docs/DECISIONS.md) K10. Terminal ve sunucu yüzeyi
  İngilizce kalıyor.
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
- **Kullanıcının göreceği bir değişiklik yaptıysan changelog girdisi yaz**:
  `crates/changelog/changelog.json`, ilgili bileşenin `unreleased` listesine,
  beş dilde. Commit mesajı yerine geçmez — commit koda ne yaptığını, changelog
  kullanıcıya ne değiştiğini anlatır (K11). `CHANGELOG.md` üretiliyor, elle
  düzenleme; bayat kalırsa CI kırılıyor. Kurallar
  [crates/changelog/README.md](crates/changelog/README.md).
- `cargo clippy` uyarısı bırakma; CI `-D warnings` ile çalışıyor.
- **İki ayrı liste var ve biri geçici.** Kökteki `TODO.md` gerçek liste, depoda.
  `tasks/` (`todo.md`, `lessons.md`) gitignore'da — oturum çalışma notları,
  otorite değil. Bir işi "kapandı" diye işaretlerken `TODO.md`'ye yaz.

## Depoda duran Claude araçları

Bu dosyadaki kuralların bir kısmı artık `.claude/` altında kendini uyguluyor
(gerekçe: `9f09332`). Hepsi depoda, klonla birlikte geliyor.

| Araç | Ne zaman |
|------|----------|
| `invariant-guard` (ajan) | Yukarıdaki değişmezlere dokunan her diff: scan-core, store, diff, dupes, treemap |
| `downstream-api-guard` (ajan) | Genel API değişti ve `main`'e push edilecek — desktop ile hub'ı buradaki CI görmüyor |
| `code-reviewer` (ajan) | Sıradan gözden geçirme, commit'ten önce |
| `test-writer` (ajan) | Yeni test; depodaki üslubu okuyup eşliyor |
| `changelog-entry` (beceri) | Kullanıcıya görünen değişiklik: beş dilde girdi, sonra `CHANGELOG.md` üretimi |
| `release` (beceri) | Sürüm kesme; tam sıra [docs/RELEASING.md](docs/RELEASING.md) |
| `preflight` (beceri) | Push öncesi CI'ın koştuğu her şey, ucuz olan önce |

**`preflight`'ı model kendi çağıramaz** (`disable-model-invocation`), kullanıcı
`/preflight` yazar — o yüzden `main`'e push etmeden önce çalıştırılmasını öner.

İki hook `.claude/settings.json` ile devrede: `CHANGELOG.md`'ye Edit/Write
bloklanıyor (üretilen dosya; Bash yönlendirmesi bilerek serbest, sürüm
prosedürü onu kullanıyor), ve oturum sonunda `crates/*/src` değişmişken
`changelog.json` değişmemişse oturum başına bir kez soruluyor. Üçüncüsü,
`.claude/hooks/rustfmt-on-edit.sh`, depoda duruyor ama yalnızca izlenmeyen
`settings.local.json`'dan bağlanıyor — bir klonun rustfmt ve jq kurmuş olmasını
şart koşmasın diye. Kendi makinende istiyorsan bağlantıyı sen ekle.

## Crate'ler

| Crate | Sorumluluk |
|-------|------------|
| `scan-core` | Tarama, ağaç modeli, platforma özel metadata. Hiçbir şeye bağlı değil. |
| `store` | SQLite anlık görüntü deposu, ncdu uyumlu dışa aktarım |
| `diff` | İki görüntüyü karşılaştırma, "suçlu klasör" tespiti |
| `dupes` | Aynı içerikli dosyalar: boyut → ön-hash → blake3, önbellek trait'i |
| `cli` | `spacetrace` ikilisi (uzak kaynaklar dâhil) |
| `agent` | `spacetrace-agent` ikilisi: zamanlayıcı + HTTP servisi |
| `treemap` | Squarified yerleşim + LOD + hiyerarşik hit-test (masaüstü kullanır) |
| `changelog` | Üç bileşenin changelog'u, beş dilde; üreteç aynı crate'in ikilisi |
| `buildinfo` | İkiliye commit, derleme tarihi ve kanal damgası (`build.rs`) |

Bağımlılık yönü tek yönlü. Ajan `scan-core`, `store` ve `buildinfo`'ya
bağlanır; `cli`'ye ve `diff`'e bağlanmaz.

## Depolar

Dört fazın hepsi çalışıyor. K2 gereği kod üç depoda:

| Depo | İçerik | Görünürlük |
|------|--------|------------|
| bu depo | yukarıdaki dokuz crate'in hepsi | public, Apache-2.0 |
| [spacetrace-desktop](https://github.com/unalcakir28/spacetrace-desktop) | Tauri v2 + React masaüstü | private, ticari |
| [spacetrace-hub](https://github.com/unalcakir28/spacetrace-hub) | Filo panosu, trend, uyarılar | private, ticari |

Diğer ikisi bu depoyu **git bağımlılığı** olarak kullanıyor, path değil. Yani
buradaki genel API'yi bozan bir değişiklik onları sessizce kırar; `main`'e
push etmeden önce bunu düşün.

## Sürüm ve site

Tam anlatım [docs/RELEASING.md](docs/RELEASING.md); kolay bozulan kısımlar:

- **Üç bileşenin indirilebilir dosyaları da bu deponun release'lerinde.**
  Masaüstü ve hub kendi CI'larında derleyip buraya yayınlıyor (`RELEASE_TOKEN`
  sırrı ile — bir depodaki `GITHUB_TOKEN` başka depoya yazamıyor, depo public
  olsa da). Etiketler: `continuous` / `v*` (CLI), `desktop-continuous` /
  `desktop-v*`, `hub-continuous` / `hub-v*`.
- **Etiket ve varlık adları sabit sözleşme, ve karşı taraf artık ayrı bir
  depoda.** Site deposundaki `src/data/releases.ts` bunlara doğrudan bağlanıyor
  ve `install.sh` dosya adını verilen sürümden kuruyor. Yeniden adlandırmak
  indirme sayfasını sessizce kırar — ve iki ayrı depo olduğu için tek bir CI
  adımı bunu yakalamıyor, aynı gün iki commit gerekiyor.
- **Konteyner imajları önceden derlenmiş musl ikililerinden kuruluyor**
  (`.github/docker/Dockerfile.release`), kökteki `Dockerfile`'dan değil. QEMU
  altında Rust derlemek arm64 imajını dakikalar yerine on dakikalar sürdürüyor.
  Kökteki Dockerfile duruyor çünkü `docker build .` bir klonda çalışsın diye var.
- **Site bu depoda değil.**
  [unalcakir28/spacetrace-website](https://github.com/unalcakir28/spacetrace-website)
  — Astro, beş dil, `spacetrace.teknobakkall.com`. Nasıl çalıştığı o deponun
  `CLAUDE.md`'sinde; burada bilmen gereken tek şey yukarıdaki indirme
  sözleşmesi. Ayrılma gerekçesi docs/RELEASING.md → Site.
- Sürüm iş akışı belge değişikliklerinde çalışmıyor (`paths-ignore`).

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
- **Dışarıdan gelen bir ağaç `Tree::from_nested`'den geçer.** ncdu içe
  aktarımı (ve sonra gelecek her format) kendi arena düzenini kurmaz: o yol
  yürüyüşün `TreeBuilder` + `aggregate`'ini çağırır, yoksa değişmez 2'nin ve
  toplamanın ikinci bir uygulaması doğar. **Dizinin kendi `asize`'ı atılır**
  (değişmez 1) — gerçek ncdu onu yazıyor, bizim dışa aktarıcımız yazmıyor,
  yani kendi çıktımızla gidiş-dönüş testi bu hatayı göremiyor.
  **Ve testi store'dan geçir.** Kökün ebeveyni `NO_PARENT` olmak zorunda;
  `0` yazmak bellekte kusursuz görünen ama `save` → `load` turunu
  `RootHasParent` ile düşen bir ağaç üretiyor ve `remove_subtree`'yi sonsuz
  döngüye sokuyor. İki ağacı bellekte karşılaştıran bir test bunu göremez —
  11 Eylül 2026'da göremedi ve kırık `import` yayınlandı.
- **`store::load` bir güven sınırı.** Uzaktan indirilen snapshot da bu yoldan
  geçiyor, bu yüzden `TreeAssembler::finish` arena değişmezlerini doğruluyor
  (`Tree::check`).
  Doğrulamayı atlayan bir yol ekleme: bozuk `children_start` indeks panic'i,
  geriye dönük bir çocuk işaretçisi sonsuz döngü demek.
- **macOS'ta listeleme iki yoldan geçiyor, ve ikisi aynı rakamı vermek
  zorunda.** `bulk.rs` `getattrlistbulk` ile adları ve metadata'yı tek çağrıda
  alıyor (ölçüldü: uçtan uca 2,3–2,4×); `read_dir` + `lstat` yolu hem
  fallback hem mount içeren dizinlerin tek yolu (D1'in girdi başına koruması
  toplu çağrıda işlemiyor). **İkinci bir metadata kaynağı sessizce
  ayrışırsa** yıllar sonra "snapshot bozuk" diye çıkıyor —
  `assert_same_answer_as_lstat` iki yolu alan alan karşılaştırıyor, yeni bir
  alan eklerken oraya da bak. Dizin `nlink`'i özellikle: `ATTR_DIR_LINKCOUNT`
  APFS'te 1, `st_nlink` 2+altdizin, ve eşitlemek için dizin başına bir
  `lstat` ödeniyor (ölçüldü: maliyeti yok).
- **Yapı doğrulaması değer doğrulaması değil, ve ikincisi `content_hash`.**
  Bir `size` alanındaki bit dönmesi kusursuz bir ağaç bırakır ve yanlış rakam
  raporlar — `Tree::check` bunu göremez. Şema v3'ten beri her tarama
  kendi mantıksal içeriğinin SHA-256'sını taşıyor
  (`crates/store/src/digest.rs`); `import_snapshot` gelen satırlardan yeniden
  hesaplayıp tutmazsa **hiçbir şeyi** içe aktarmıyor, `export_snapshot`
  bozuk olduğunu bildiği veriyi göndermiyor, `spacetrace verify` istendiğinde
  bakıyor. `NULL` = "özet yok" (v3 öncesi snapshot), "bozuk" değil.
  **Kimlik doğrulaması değil:** gövdeyi değiştirebilen özeti de yeniden
  hesaplar. Tehdit modeli bozulma, saldırgan değil.
- **`scans` tablosuna eklenen her sütun hem `create_tables`'ın hem
  `migrate_from`'un sonuna, aynı sırayla.** `export_snapshot`
  `INSERT INTO snap.scans SELECT * FROM main.scans` yapıyor ve `ALTER TABLE`
  yalnızca sona ekleyebiliyor; sıralar ayrışırsa kopya her değeri yanlış
  sütuna yazar ve hiçbir şey söylemez. Testi
  `crates/store/tests/integrity.rs` içinde.
- Bir kök aynı anda yalnızca bir kez taranır (`Runner::try_claim`, HTTP'de 409).
- Zamanlayıcı UTC + sabit offset ile çalışır; saat dilimi veritabanı yok.
- Kapasite **boş/toplam** olarak raporlanır, "% dolu" olarak değil (K6).
  Modül dışına `capacity_of` adıyla açılıyor (`capacity::of` değil).
- **`ScanProgress` yalnızca sayaç değil, iptal anahtarı da.** Yürüyüş dizin
  başına bir kez kontrol ediyor — girdi başına kontrol en sıcak döngüye paylaşımlı
  bir atomik okuma koyardı ve okunmuş bir dizini bırakmak hiçbir şey kazandırmaz.

## Bilinen eksikler

Bunlara denk gelirsen bug değil, bilinen borç (tam liste TODO.md'de):

- Windows'ta `alloc` ve hardlink dedupe **yazıldı, doğrulaması yalnızca CI'da**
  (9 Eylül 2026). Girdi başına **bir handle** açılıyor (`std::fs::OpenOptions`,
  `FILE_READ_ATTRIBUTES` + `BACKUP_SEMANTICS` + `OPEN_REPARSE_POINT`) ve o
  handle üstünden `FILE_STANDARD_INFO` → `AllocationSize`,
  `BY_HANDLE_FILE_INFORMATION` → nlink + file id + volume okunuyor.
  **`GetCompressedFileSizeW` kullanmayı denemeyin** — sıkıştırılmamış ve sparse
  olmayan dosyalarda mantıksal boyutu döndürüyor; CI 100.001 baytlık dosyaya
  100.001 dedi. Maliyet: girdi başına fazladan çağrı, ölçülmüş mertebe +36%;
  kaldıran şey B4 (`NtQueryDirectoryFileEx`). `FileIdentity::Skipped` ikinci
  sorguyu atlıyor, handle'ı değil.
- **btrfs/ZFS'te** reflink ve sıkıştırma yüzünden ağaç yürüyüşü gerçek
  kullanımı yanlış raporluyor. **APFS clone'ları tekilleştiriliyor** (A3,
  9 Eylül 2026, `fcntl(F_LOG2PHYS_EXT)`, varsayılan açık) — bu satır 11 Eylül'e
  kadar tersini söylüyordu ve yukarıdaki değişmez 1 ile çelişiyordu; ikisi
  aynı dosyada.
- Tarama tüm ağacı bellekte tutuyor. Ağacın kendisi **96 bayt/girdi**,
  100k–10M arası doğrusal ve iki platformda aynı (10M = 916 MiB).

  **Çift depolama 14 Eylül 2026'da kalktı (B1-K)** ve ölçüm o gün yeniden
  yapıldı: `/Applications` tepe 91,5 → 57,6 MB, ipucu verilince 221 → **125
  bayt/girdi**; Linux'ta "ağaç olmayan" kısım yarıya indi. Ölçüm aracı depoda:
  `cargo run --release -p spacetrace-scan-core --example memprobe -- scan <kök>`
  ve karşılaştırma için `scripts/bench-walk.sh`.

  **Kalan fark platforma bağlı ve macOS'ta hâlâ birikiyor.** Linux'ta tepe
  ağacın ~1,5 katı ve tekrarlı taramalarda neredeyse düz; macOS'ta libmalloc
  parçalanmış span'ları geri vermiyor, o yüzden tepe ayırma trafiğiyle
  büyüyor — ve trafik korpusa bağlı: `/Applications` 8 taramada +7 MiB,
  `~/github` (aynı girdi sayısı, üç katı ad baytı) 8 taramada 205 → 445 MiB.
  Sızıntı değil; `malloc_zone_pressure_relief` hiçbir şey değiştirmiyor.
  Ajan etkilenmiyor (Linux). Masaüstünde "Yeniden tara" etkileniyor, ama
  artık `expected_entries` ipucunu geçiyor. Tam ölçüm TODO.md D4.
- Ajanda yerleşik TLS yok; ters vekil öneriliyor. **Hız sınırlama var**
  (14 Eylül 2026, `ratelimit.rs`): istemci adresi başına token bucket,
  auth'tan **önce** — `/health` tokensiz ve yanlış bir token da cevaba mal
  oluyor, yani sınırlanması gereken trafik tam olarak tokenin dışında kalanı.
  Ters vekil arkasında her istek vekilin adresinden geldiği için tek ortak
  sınıra dönüşüyor; `X-Forwarded-For` bilinçli olarak okunmuyor.

## Bu depo dışındaki bağlam

Proje Cowork'te (claude.ai) başladı; oradaki oturum hafızası Claude Code'a
aktarılmaz, iki sistem ayrıdır. Aktarılması gereken her şey bu depodaki
belgelere yazıldı. Eylül 2026 pazar ve teknik araştırmasının özeti
[docs/RESEARCH.md](docs/RESEARCH.md) içinde.
