# Mimari

Bu belge kodun nasıl kurulduğunu ve **neden öyle kurulduğunu** anlatır. Ürünün
gerekçesi için [WHY.md](WHY.md), plan için [ROADMAP.md](ROADMAP.md).

## Genel şema

```
┌──────────────────┐  ┌──────────────┐  ┌───────────────────┐
│ Masaüstü (Tauri) │  │ CLI          │  │ Merkez (Faz 4)    │
│ Faz 3            │  │ ✅ Faz 1     │  │ fleet panosu      │
└────────┬─────────┘  └──────┬───────┘  └─────────┬─────────┘
         │ süreç içi         │ doğrudan            │ HTTP
         │                   │                     │
         │            ┌──────┴──────────┐   ┌──────┴──────┐
         │            │ Ajan (Faz 2)    │   │  Ajanlar    │
         │            │ serve / push    │   │  n makine   │
         │            └──────┬──────────┘   └─────────────┘
┌────────┴──────────────────┴────────────────────────────────┐
│  Rust çekirdeği — üç kabuk da aynı kodu kullanır           │
│  scan-core · store · diff                                   │
└─────────────────────────────────────────────────────────────┘
```

Tek çekirdek, birden çok paket. Masaüstü uygulaması, ajan ve CLI aynı tarama ve
karşılaştırma kodunu çalıştırır; aralarındaki fark yalnızca arayüz ve ağ
katmanıdır. Referans olarak Czkawka'nın `czkawka_core`'u CLI, GTK4 ve Slint
arayüzleri tarafından aynı şekilde paylaşılıyor.

## Crate'ler

| Crate | Sorumluluk | Bağımlı olduğu |
|-------|------------|----------------|
| `scan-core` | Dizin taraması, ağaç modeli, platforma özel arka uçlar | rayon |
| `store` | SQLite anlık görüntü deposu, ncdu dışa aktarım | scan-core, rusqlite |
| `diff` | İki anlık görüntüyü karşılaştırma | scan-core |
| `cli` | `spacetrace` ikilisi | hepsi, clap |

Bağımlılık yönü tek yönlü: `scan-core` hiçbir şeye bağlı değil, `store` ve `diff`
yalnızca ona bakar. Ajan (Faz 2) bu üçünü kullanacak ve `cli`'ye bağlanmayacak.

## Ağaç modeli: neden arena

Düğüm başına `Vec<Child>` tutan bir ağaç, milyonlarca dosyada hem bellek hem de
işaretçi takibi yüzünden pahalıdır. Bunun yerine tek bir `Vec<Node>` kullanılır
ve düğümler **BFS sırasıyla** yerleştirilir. Bunun üç sonucu var:

1. Bir düğümün çocukları **bitişik** bir indeks aralığındadır
   (`children_start .. children_start + children_len`), yani düğüm başına ayrı
   bir liste ayırmaya gerek yok.
2. Her çocuk, ebeveyninden **büyük** bir indekse sahiptir. Alt ağaç toplamlarını
   hesaplamak bu yüzden tek bir ters geçiştir (`aggregate`), özyineleme yok.
3. Treemap yerleşimi ve çizimi diziyi sırayla tarar — önbellek dostu.

Referans: ncdu 2 dosya başına ~25 bayt ile 3.8M dosyayı 162 MB'da tutuyor; hedef
bu mertebe.

## Tarama

Yürüyüş **paralel DFS**'tir: bir dizinin içeriği tek iş parçacığında okunur
(çekirdek ardışık `readdir` için en hızlıdır), sonra alt dizinler rayon
havuzuna dağıtılır. Böylece SSD meşgul tutulurken BFS kuyruğunun bellek şişmesi
yaşanmaz.

Kararlar:

- **Sembolik bağlantılar izlenmez.** `DirEntry::metadata()` bağlantıyı takip
  etmez; bağlantı kendi boyutuyla sayılır. Bu hem döngü riskini sıfırlar hem de
  bir ağacın iki kez sayılmasını engeller.
- **Sabit bağlantılar bir kez sayılır.** `nlink > 1` olan dosyalar için
  `(dev, ino)` çifti paylaşımlı bir kümede tutulur; ikinci kez görülen kopya
  ağaçta görünür kalır ama 0 bayt katkı yapar. `--no-dedupe` ile kapatılabilir.
- **Hatalar yutulmaz.** Okunamayan her yol sayılır, ilk 64 tanesi yolu ve hata
  mesajıyla saklanır. Bir izin hatası taramayı durdurmaz.
- **`one_filesystem`** kök ile aynı `dev` değerine sahip olmayan dizinlere
  inmez (`du -x` davranışı).

## Boyut anlambilimi

İki ayrı büyüklük raporlanır ve asla karıştırılmaz:

| Alan | Anlamı | Karşılığı |
|------|--------|-----------|
| `size` | Mantıksal boyut — yalnızca **dosya** baytları | `du -sb` |
| `alloc` | Diskte tahsis edilen bloklar, **dizin blokları dâhil** | `du -s --block-size=1` |

Dizinlerin kendi inode boyutu (`len()`, tipik olarak 4096) mantıksal toplama
**girmez**: kullanıcı "bu klasördeki dosyalar ne kadar yer tutuyor" beklerken
dizin defterlerinin eklenmesi rakamı açıklanamaz kılar. Ama bu bloklar diskte
gerçekten yer kapladığı için `alloc`'a dâhildir.

Unix'te `alloc`, `st_blocks * 512` üzerinden hesaplanır (POSIX'e göre birim her
zaman 512 bayttır, dosya sisteminin blok boyutundan bağımsız). Bu sayede seyrek
(sparse) dosyalar mantıksal boyutlarından az, küçük dosyalar ise blok
yuvarlaması yüzünden çok görünür — ikisi de doğrudur.

**Doğrulama:** `/usr` (141k dosya), `/usr/share` ve `/etc` üzerinde her iki
toplam da `du` ile tam olarak eşleşiyor. Bu bir test koşuludur.

## Anlık görüntü deposu

Arena düzeni SQLite'a **olduğu gibi** yazılır: `entries.id` düğümün indeksidir,
`children_start`/`children_len` korunur. Sonuç: bir anlık görüntüyü yüklemek
`ORDER BY id` ile tek sıralı sorgudur, ağaç yeniden kurulmaz.

```sql
scans(id, host, root, started_at, duration_ms, total_size, total_alloc,
      files, dirs, errors, hardlinks_deduped, scanner_version, label)

entries(scan_id, id, parent_id, name, kind, size, alloc, mtime, nlink,
        files, dirs, children_start, children_len)   -- WITHOUT ROWID
```

`host` + `root` bir **hedefi** tanımlar; karşılaştırma ve `prune` bu ikili
üzerinden çalışır. `PRAGMA user_version` şema sürümünü tutar; daha yeni bir
sürümle yazılmış veritabanı açılmaz, sessizce yanlış okunmaz.

Ayrıca ncdu uyumlu JSON dışa aktarımı vardır. Sebep pratik: sunucudan aldığın
bir kaydı henüz masaüstü uygulaması yokken `ncdu -f scan.json` ile
inceleyebilmek.

## Karşılaştırma: "suçlu klasör"

Naif bir diff, değişen her yolu listeler — bir haftalık çalışmadan sonra binlerce
satır. İşe yarayan soru "hangi yollar değişti" değil, **"yer nereye gitti"**.

Algoritma kökten aşağı iner ve her dizinde şunu sorar: *bu değişimin neredeyse
tamamını tek bir alt klasör mü açıklıyor?* Cevap evetse (varsayılan eşik %90) o
klasöre inilir; hayırsa değişim gerçekten burada dağılmıştır ve bu seviye
raporlanır. Yalnızca eklenen veya silinen ağaçlar tek satırda, en üst
seviyelerinde bildirilir.

Pratikte:

```
   +40.1 MiB  büyüdü    42.9 MiB  app/logs/     ← app/ ve kök atlandı
   +14.3 MiB  büyüdü    25.7 MiB  backups/
    -1.9 MiB  silindi         0 B  uploads/      ← alt ağaç tek satır
```

Çocuklar isme göre **merge-join** ile eşleştirilir (her iki taraf sıralanır),
yani tüm ağacın hash haritası bellekte tutulmaz.

## Platforma özel arka uçlar

Şu an tek bir taşınabilir arka uç var (`read_dir` + `symlink_metadata`) ve
metadata okuma `cfg` ile ayrılmış durumda. Planlanan hızlı yollar:

| Platform | Yöntem | Not |
|----------|--------|-----|
| Windows | NTFS **MFT** doğrudan okuma; **USN journal** ile artımlı | Yönetici hakkı gerekir; ReFS'te MFT yok. Faz 3'te zorunlu: WizTree 2 TB'ı ~14 s'de tarıyor. |
| Windows (yetkisiz) | `NtQueryDirectoryFileEx`, 64 KB buffer, `FileIdBothDirectoryInformation` | Boyut + file id tek çağrıda; hardlink dedupe için ek syscall gerekmez |
| macOS | `getattrlistbulk` | Boyut/tarih gerektiğinde `readdir + lstat`'tan belirgin hızlı |
| Linux | `getdents64` + `statx`, thread başına DFS | Mevcut yaklaşım zaten bu modelde |

`scan-core` içindeki `RawMeta::from_metadata` bu ayrımın sınırıdır; hızlı yollar
aynı `RawMeta`'yı üreterek devreye girecek.

**Bilinen eksik:** Windows'ta `alloc` şu an mantıksal boyuta eşitleniyor ve
hardlink tekilleştirme kapalı (`nlink = 1`). Gerçek değerler için
`GetFileInformationByHandleEx` (FILE_STANDARD_INFO) ve `FileIdInfo` gerekiyor;
kod içinde `TODO(win)` ile işaretli.

## Neden bu teknoloji seçimleri

**Rust.** Ajanın tek statik ikili olarak NAS'a ve konteynere kurulabilmesi
gerekiyor; runtime bağımlılığı olmayan bir dil şart. Ayrıca düşük seviye
dosya sistemi çağrıları (MFT, getattrlistbulk) için hazır crate ekosistemi en
geniş burada.

**Tauri v2 (Faz 3).** Mobil kapsamdan çıkınca Flutter'ın "beş platform tek kod"
avantajı ortadan kalktı. Tauri masaüstünde olgun, kurulum dosyası 3–15 MB
(Electron'da 50–150 MB), ve Rust çekirdeğini süreç içinde doğrudan çağırır —
FFI köprüsü yok. Arayüz React/TS olduğu için mevcut web bilgisi doğrudan
kullanılabilir. Riski Linux'taki WebKitGTK farklılıkları; treemap üç WebView'da
da test edilmeli.

**Yedek plan: Avalonia 12 + .NET.** Rust maliyeti kabul edilemez bulunursa
masaüstü ve ajan uçtan uca C# yazılabilir (NativeAOT ile tek ikili). Mimari ve
ürün tanımı değişmez.

**SQLite.** Anlık görüntüler dosya olarak taşınabilir olmalı (sunucudan `scp` ile
al, yerelde aç). Snapshot diff'i bir sorguya dönüşür. Sunucu kurulumu gerekmez.

## Bilinen sınırlar ve teknik borç

- Windows `alloc` ve hardlink desteği eksik (yukarıda).
- btrfs/ZFS'te reflink, sıkıştırma ve dedup yüzünden ağaç yürüyüşü gerçek disk
  kullanımını yanlış raporlar. Doğrusu için `btdu` gibi örnekleme gerekir;
  şimdilik "dosya sistemi farkında mod" Faz 5'te.
- APFS clone'ları henüz tekilleştirilmiyor (macOS'ta `alloc` şişebilir).
- Tarama tüm ağacı bellekte tutar. 10M+ dosyalı köklerde bellek profili
  ölçülmeli; gerekirse akışlı yazma eklenecek.
- `Tree::rel_path` her çağrıda kökten yukarı yürür; derin ağaçlarda sıcak
  döngüde kullanılmamalı.
- Arayüz dizeleri Türkçe ve koda gömülü; i18n yok (bkz. ROADMAP, lansman öncesi
  zorunlular).

## Geliştirme

```bash
cargo test --workspace                       # 32 test
cargo clippy --workspace --all-targets       # uyarısız olmalı
cargo fmt --all
cargo check -p spacetrace-scan-core --target x86_64-pc-windows-msvc
```

Testler geçici dizinlerde gerçek dosya sistemi kullanır: sabit bağlantı,
sembolik bağlantı, izin hatası, derinlik sınırı ve diff senaryoları dâhil.
Yeni bir tarama davranışı eklerken `du` ile karşılaştırma testi de eklenmelidir.
