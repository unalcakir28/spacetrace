# Rakip analizi (Eylül 2026)

9 Eylül 2026'da yapılan rakip araştırmasının ve **kendi makinemizde alınan
ölçümlerin** kaydı. [RESEARCH.md](RESEARCH.md) Eylül 2026 başındaki pazar
araştırmasını tutuyor; bu belge onun rakiplere odaklı, ölçümle desteklenmiş
devamı.

> Rakip sürümleri ve fiyatları hızla değişir. Bir karar bunlardan birine
> dayanıyorsa kullanmadan önce kaynağı yeniden kontrol et.

## Kanıt etiketleri

Bu belgedeki her iddia şunlardan biriyle işaretli. Karıştırmamak önemli: bir
satıcının kendi hız iddiası ile ikilisinden okunan bir API adı aynı ağırlıkta
değil.

| Etiket | Anlamı |
|--------|--------|
| `[ölçüm]` | Bu makinede biz ölçtük, aşağıdaki yöntemle tekrarlanabilir |
| `[ikili]` | Rakibin ikilisinden/kaynak kodundan okundu |
| `[satıcı]` | Satıcının kendi ifadesi |
| `[kullanıcı]` | Üçüncü taraf kullanıcı raporu, kontrollü değil |
| `[bulunamadı]` | Arandı, kaynak bulunamadı — **tahmin yürütülmedi** |

---

## 1. Kendi ölçümlerimiz

**Ortam.** Mac15,9 (Apple Silicon, 16 çekirdek), 48 GB RAM, macOS 26.5.2, APFS.
Hedef `/Applications` = 412.233 girdi (33.661 dizin + 378.572 dosya; `find` ile
birebir doğrulandı). Sıcak dosya sistemi önbelleği, 5 tekrarın en iyisi.
Soğuk önbellek ölçülmedi (`purge` root gerektiriyor).

### 1.1 Yavaşlığın payları

Bir disk tarayıcısını yavaşlatan iki mekanizmayı izole etmek için aynı makinede
dört konfigürasyon ölçüldü `[ölçüm]`:

| Konfigürasyon | Süre |
|---------------|------|
| Tek thread + girdi başına fazladan `stat` (Python) | 8.03 s |
| Tek thread, fazladan syscall yok (Python) | 5.17 s |
| `du -s` (tek thread, C) | 1.19 s |
| spacetrace, `RAYON_NUM_THREADS=1` | 5.79 s |
| **spacetrace, 8 thread** | **1.11 s** |

Çıkarılan iki pay:

- **Paralellik: 5.2×** (5.79 s → 1.11 s, aynı ikili, tek değişken thread sayısı)
- **Girdi başına fazladan `stat`: +36%** (5.17 s → 8.03 s, aynı dil, aynı tek
  thread)

İkisi birlikte **7.2×**. 400k girdide 7 saniye; 4M dosyalı bir diskte 70 saniyeye
karşı 10 saniye.

**Önemli ve sezgiye aykırı sonuç:** spacetrace tek thread'e indirildiğinde
(5.79 s) Python'dan (5.17 s) **daha yavaş**. Bu iş CPU-bound değil,
**syscall-bound** — zaman çekirdekte `stat` yapmakla geçiyor. Yani bir disk
aracının hızı **dil seçiminden değil, paralellikten ve girdi başına syscall
sayısından** geliyor. "Rust olduğu için hızlı" savunulabilir bir iddia değil;
"paralel yürüdüğü için hızlı" ölçülmüş bir iddia.

### 1.2 Thread ölçeklenmesi

| Thread | Süre | Hızlanma |
|--------|------|----------|
| 1 | 5.79 s | 1.0× |
| 2 | 2.37 s | 2.4× |
| 4 | 1.58 s | 3.7× |
| 8 | **1.11 s** | **5.2×** |
| 16 | 1.27 s | 4.6× ← **gerileme** |

16 thread'te gerileme gerçek: 412k girdide iş parçası başına maliyet küçüldüğü
için rayon'un iş-çalma koordinasyonu baskın gelmeye başlıyor.

**10 Eylül 2026, ikinci ölçüm — ve en iyi thread sayısı diye bir şey yok.**
Yukarıdaki tablo tek bir korpusta (412k girdi) alınmıştı. İki korpusta
tekrarlandığında optimumun ağacın büyüklüğüyle *kaydığı* görüldü. M3 Max
(12 performans + 4 verimlilik çekirdeği), serpiştirilmiş koşu, 9 örneğin
medyanı `[ölçüm]`:

| Korpus | 6 | 8 | 10 | 12 | 16 |
|--------|---|---|----|----|----|
| `/usr`, 50k girdi | **71 ms** | 92 | 106 | 139 | 151 |
| `/Applications`, 412k girdi | 1541 | 1407 | 1426 | **1233 ms** | 1581 |

Küçük ağaçta 6, büyük ağaçta 12 kazanıyor; 16 ikisinde de sonuncu.
**Varsayılan artık `min(çekirdek, 8)`** — hiçbir korpusta en iyi değil (küçükte
%30, büyükte %14 geride) ama ikisinde de eski varsayılanı yeniyor (%39 ve %11).
Sabit bir sayının en iyi olması zaten mümkün değildi; `--threads` bilen
kullanıcı için duruyor.

**Yöntem notu.** İlk denemede ayarlar sırayla ölçüldü ve arka planda kalan bir
tarama yüzünden aynı ayar iki koşuda 89 ms ve 170 ms verdi — o veri atıldı.
Makine sessizleşmediği için ölçüm sessizliğe değil, serpiştirmeye dayandırıldı:
her turda bütün ayarlar rastgele sırayla koşuyor, böylece sürüklenme hepsine
eşit dağılıyor. Tur toplamları ±%5 içinde kaldı.

**Bir de işe yaramayan deney.** Aralığı 1.2M girdiye taşımak için sentetik bir
ağaç üretildi ve sonucu kullanılmadı: üreteç dizinleri birbirinin altına
zincirlediği için derin ve dar bir ağaç çıktı, o da paralelleşmiyor. Dizin
başına dosya sayısı tutturulmuştu ama paralellik için asıl önemli olan
**dallanma** kaçırılmıştı. 412k üstü hâlâ ölçülmedi.

### 1.3 Doğruluk

`/usr/share` (19.288 dosya, 892 dizin, 0 hata) `[ölçüm]`:

```
du -s          → 265.641.984 bayt
total_alloc    → 265.641.984 bayt   ← bit birebir
total_size     → 524.146.086 bayt
```

**Düzeltme (9 Eylül 2026):** bu ölçüm **elle** alınmıştı ve ilk hâli "169 test
geçiyor, bu değişmez #1'in canlı kanıtı" diyordu — yanlıştı. O 169 testin
hiçbiri `du`'yu çağırmıyordu; `totals_match_the_files_on_disk` testin kendi
yazdığı sabitlerle karşılaştırıyor ve `alloc` için tek iddiası `>= 4096`.
Otomatik karşılaştırma **aynı gün yazıldı**:
`crates/scan-core/tests/du_equivalence.rs` (6 test, üç platformda CI'da koşuyor,
Windows'ta A1/A2 beklemede). Doğrulaması mutasyon testiyle yapıldı: `alloc`'u
mantıksal boyuta eşitlemek 3 testi, dedupe'u bozmak 3 testi, dizin inode
boyutunu mantıksal toplama eklemek 1 testi düşürüyor. Yani iddia artık
kanıtlanmış — ama **9 Eylül'e kadar değildi.**

### 1.4 Bellek — hedefin çok üstünde

`Node` = **104 bayt** (`size_of::<Node>()` ile ölçüldü), hizalama 8.

| Hedef | Girdi | Tepe RSS | Girdi başına (taban çıkarılmış) |
|-------|-------|----------|--------------------------------|
| taban (küçük dizin) | — | 8.1 MB | — |
| /usr | 50.132 | 26 MB | ~359 B |
| ~/github | 120.065 | 61 MB | ~437 B |
| /Applications | 412.233 | 122 MB | ~276 B |

104 baytın üstüne binen iki maliyet: düğüm başına ayrı heap'e giden
`name: String` (24 bayt gövde + ayrı tahsis + malloc başlığı) ve arena
`Vec`'inin ikiye katlanarak büyümesi (realloc anında eski + yeni tampon birlikte
yaşıyor).

[RESEARCH.md §3](RESEARCH.md) hedefi **~25 B/dosya** (ncdu 2 mertebesi).
Ölçülen 276–437 B/girdi, yani hedefin **11–17 katı**. 10M dosyaya ekstrapole:
**~2,8 GB tepe bellek.**

**Düzeltme (9 Eylül 2026):** o hedef bizim alan kümemizle **ulaşılabilir değil**
ve karşılaştırma elmayla armut — ncdu 2 düğüm başına `own_size`, `own_alloc`,
`files`, `dirs` tutmuyor, biz tutuyoruz. Faz faz RSS probuyla alınan dağılım
`[ölçüm]`:

| Kalem | B/girdi |
|-------|---------|
| `RawEntry` ara ağacı (yürüyüş fazı) | 88 |
| arena `Node` | 104 |
| ad `String`'leri | ~32 |
| parçalanma + malloc başlıkları | ~46 |
| taban | — |
| **toplam** | **290** |

Yürüyüş bittiğinde RSS 77 MB, flatten bittiğinde 119 MB: **ara ağaç ile arena
aynı anda yaşıyor** ve serbest bırakılan `RawEntry` belleği işletim sistemine
geri dönmüyor. Yani 290'ın 192'si çift depolama, asıl iş orada. En agresif alan
daraltmasıyla bile taban `Node` 72 B + ad ~21 B = **~93 B/girdi**. Gerçekçi
hedef bu yüzden **dua-cli'nin 64 B'ı** mertebesi, ncdu'nun 25 B'ı değil.

Karşılaştırma noktaları:

| Araç | Düğüm/girdi başına | Kaynak |
|------|--------------------|--------|
| ncdu 2.0 | dosya **25 B**, dizin **56 B** | `[satıcı]` doğrulandı |
| ncdu 1.16 | dosya 78 B, dizin 78 B | `[satıcı]` |
| dua-cli 2.44.0 | **64 B** arena düğümü | `[satıcı]` |
| **spacetrace** (9 Eyl, önce) | ~276–437 B | `[ölçüm]` |
| **spacetrace** (9 Eyl, sonra) | **231 B** (`Node` 72 B) | `[ölçüm]` |

`dua-cli` 2.44.0 çözümü açıkça yazmış: **64 baytlık arena düğümü + paylaşılan
dosya adı deposu + yoğun dizin id'leri**, tepe RSS %49 aşağı (525 MB → 268 MB).
Bizim 104 bayt + düğüm başına `String` tasarımımız tam olarak onların terk ettiği
tasarım.

---

## 2. Rakip yığınları

| Ürün | Dil / Yığın | Numaralandırma | Paralellik |
|------|-------------|----------------|------------|
| TreeSize Free/Pro | **Delphi/VCL** `[satıcı]` | Yönetici ise **MFT**; değilse normal `[satıcı]` | 2 thread (Pro'da 32), CPU yüküne göre `[satıcı]` |
| WizTree | `[bulunamadı]` | **MFT'yi diskten ham okuyor** `[satıcı]` | `[bulunamadı]` |
| WinDirStat 2.x | C++ `[ikili]` | `NtQueryDirectoryFile`; **MFT ancak v2.5.0'da (Ocak 2026) ve opsiyonel** `[ikili]` | Sürücü başına çok thread (v2.0.1) `[ikili]` |
| SpaceObServer | Delphi + MSSQL `[satıcı]` | **USN Journal** ile artımlı `[satıcı]` | Yapılandırılabilir `[satıcı]` |
| SpaceSniffer | `[bulunamadı]` | `[bulunamadı]` | `[bulunamadı]` |
| DaisyDisk | `[bulunamadı]` | `[bulunamadı]` | `[bulunamadı]` |
| GrandPerspective | Objective-C `[ikili]` | `[bulunamadı]` | `[bulunamadı]` |
| QDirStat | C++/Qt6 `[ikili]` | `readdir` + `fstatat`, **inode'a göre sıralayıp** stat `[ikili]` | Tek thread, zamanlayıcı tabanlı iş kuyruğu `[ikili]` |
| Filelight | C++/Qt `[ikili]` | `[bulunamadı]` | `[bulunamadı]` |
| Baobab | **Vala**/GTK `[ikili]` | GIO `enumerate_children_async` `[ikili]` | Olay döngüsü, thread havuzu değil |
| btdu | **D** `[ikili]` | Ağaç yürüyüşü **değil** — Monte Carlo örnekleme `[satıcı]` | Çok işlemli, io_uring varyantı var |
| ncdu 2 | **Zig** `[satıcı]` | `openat` ailesi `[satıcı]` | **Tek thread** (çok thread yol haritasında) |
| ncdu 1.x | C | `chdir` + `opendir` | Tek thread |
| gdu | Go `[ikili]` | goroutine paralel; analiz sırasında **GC kapalı** `[ikili]` | `--max-cores`, `--sequential` `[ikili]` |
| dust | Rust + rayon `[ikili]` | Standart Rust FS API `[ikili]` | rayon iş-çalma |
| dua-cli | Rust `[ikili]` | `[bulunamadı]` | "Parallel by default" `[satıcı]` |
| dut | C `[ikili]` | DFS + binary heap `[ikili]` | `[bulunamadı]` |
| diskus | Rust `[ikili]` | rayon üstüne özel yürüyücü `[ikili]` | rayon |
| erdtree | Rust `[ikili]` | `[bulunamadı]` | **Ampirik 3 thread** (1:1 çekirdek yerine) `[ikili]` |
| **FreeSize** | **.NET + Photino.Blazor** `[ikili]` | **`DirectoryInfo.GetFiles`/`GetDirectories`** `[ikili]` | **İzi yok** `[ikili]` |
| Diskaroo | Swift (mac) + WPF/.NET 8 (Win); Linux `[bulunamadı]` `[satıcı]` | **MFT okumadığını kendisi kabul ediyor** `[satıcı]` | `[bulunamadı]` |
| DiskRaptor | Rust + Tauri 2 `[ikili]` | jwalk (mac) / walkdir (Win, Linux) + `getattrlistbulk`, `FindFirstFileW` `[ikili]` | rayon, jwalk |
| **spacetrace** | **Rust** | `read_dir` + `symlink_metadata` | **rayon, ölçülen 5.2×** `[ölçüm]` |

### 2.1 Özellik matrisi

| Ürün | Geçmiş + diff | Uzak ajan | Platform | Lisans / fiyat |
|------|---------------|-----------|----------|----------------|
| **spacetrace** | **✓** | **✓** | Win/mac/Linux | Apache-2.0 çekirdek |
| SpaceObServer | ✓ tam | ✓ Windows servisi | Windows | ~$283+ `[kullanıcı]` |
| dua-cli | **✓ v2.44.0'dan beri** | ✗ | çapraz | MIT |
| FreeSize | ◐ Pro "Portal" `[satıcı]` | ◐ belirsiz | Win/mac/Linux | CHF 0 / **29/yıl** |
| TreeSize Pro | ◐ kayıtlı index karşılaştırma `[satıcı]` | ◐ UNC/SSH | Windows | ücretli |
| gdu | ◐ SQLite'a kaydet+yükle, **diff yok** `[ikili]` | ✗ | çapraz | MIT |
| QDirStat | ◐ cache dosyası (diff değil) `[ikili]` | ✗ | Linux | GPL |
| WizTree | ✗ | ✗ | Windows | $0 kişisel / $25+ iş |
| WinDirStat 2.x | ✗ | ✗ | Windows | GPL-2.0 |
| DaisyDisk | ✗ | ✗ | macOS | $9.99 |
| Diskaroo | ✗ (kendi tablosu: "Time-based Comparison: No") `[satıcı]` | ✗ | Win/mac/Linux | $19.99 kalıcı |
| DiskRaptor | ✗ (kaynak kodda snapshot deposu yok) `[ikili]` | ✗ | Win/mac/Linux | MIT |
| ncdu / dust / dut / diskus | ✗ | ✗ | çapraz | açık kaynak |
| Baobab / Filelight | ✗ | mount tabanlı | Linux | GPL |

---

## 3. FreeSize — ikili adli incelemesi

FreeSize kapalı kaynak ve teknoloji yığını hiçbir halka açık kaynakta
yazmıyor `[bulunamadı]`. macOS `.pkg`'si (35.766.335 bayt) indirilip
`pkgutil --expand-full` ile açıldı ve `FreeSize.app` doğrudan incelendi.

### 3.1 Yığın `[ikili]`

```
FreeSize.app/Contents/MacOS/
  Photino.Native.dylib, Photino.NET.dll, Photino.Blazor.dll
  libcoreclr.dylib                              ← gömülü .NET runtime
  Microsoft.AspNetCore.Components.WebView.dll   ← Blazor
  libSystem.Native.dylib, FreeSize.runtimeconfig.json
  wwwroot/{index.html, css/app.css, js/app.js}
  … 203 DLL, toplam 87 MB
```

Sürüm 0.3.1.0, `com.FreeSize.FreeSize`, imza `Developer ID Application: LightNet
(88S5ATC4M6)`, arm64-only. Windows tarafı aynı aile: `shared\Microsoft.NETCore.App`,
`Microsoft.AspNetCore.App`, `WebView2Loader.dll`, `Microsoft Edge WebView2 Runtime`.

**FreeSize = .NET + [Photino.Blazor](https://www.tryphotino.io/)** — arayüz C#'ta
Blazor ile yazılmış, işletim sisteminin kendi webview'inde çiziliyor.

Elenen alternatifler `[ikili]`: Electron değil, Qt değil, Java değil — `electron`,
`libffmpeg`, `node_modules`, `Qt5|Qt6`, `libjvm`, `v8_context` aramaları sıfır.
74 MB'lık Windows kurulumunun sebebi Chromium değil, gömülü CoreCLR.

**Not:** Mimari ailesi bizimle **aynı** — native kabuk + OS webview, tam
Tauri'nin yaptığı şey. Fark çatıda değil, çatının içinde ne yapıldığında.

### 3.2 Neden yavaş — üç mekanizma

`FreeSize.Core.dll` (85 KB, tarama çekirdeğinin tamamı) metadata taraması `[ikili]`:

| Aranan API | Bulundu |
|------------|---------|
| `GetFiles`, `GetDirectories`, `DirectoryInfo` | ✓ |
| `EnumerateFiles`, `EnumerateFileSystemInfos` | **0** |
| `Parallel`, `MaxDegreeOfParallelism` | **0** |
| `ConcurrentQueue` / `ConcurrentBag` / `ConcurrentDictionary` | **0** |
| `SemaphoreSlim`, `ThreadPool` | **0** |
| `EnumerationOptions`, `RecurseSubdirectories` | **0** |

Referans verilen assembly'ler: `System.IO`, `System.IO.FileSystem.DriveInfo`,
`System.Linq`, `System.Threading`, `System.Threading.Tasks`,
`System.Threading.Thread` — **`System.Collections.Concurrent` yok.**

**① Paralellik yok.** Paralel bir dizin yürüyüşü thread-safe bir iş kuyruğu
olmadan yazılamaz; concurrent koleksiyon referansı hiç yok. `Thread`/`Task`
varlığı Blazor arayüzünü bloke etmemek için **tek** bir arka plan tarama
thread'iyle birebir uyumlu. Satıcı da hiçbir yerde çok thread iddiası yapmıyor;
aksine WizTree'nin MFT üstünlüğünü kendisi kabul ediyor `[satıcı]`.
→ Ölçtüğümüz ceza: **5.2×**.

**② `Enumerate*` yerine `Get*`.** .NET'te `GetFiles()` dönmeden önce dizinin tüm
`FileInfo[]` dizisini malzemeleştirir; `EnumerateFiles()` tembel akıtır.
Microsoft'un kendi dokümanı performans için `Enumerate*`'i öneriyor. Her
`FileInfo` heap'te bir nesne + tam yol string'i: 400k girdide 400k nesne + 400k
string → ağır GC baskısı. `FileInfo.Length` önbelleklenmemişse dosya başına ayrı
`stat`. → Ölçtüğümüz ceza: **+36%**.

**③ Treemap DOM'a çiziliyor — muhtemelen en büyüğü.** `wwwroot`'ta canvas yok:
`getContext`, `canvas`, `requestAnimationFrame`, `d3`, `OffscreenCanvas`
aramaları **sıfır** `[ikili]`. `app.js` toplam 5.242 bayt ve tek işi kendi
yorumuyla belli:

```js
// Measure an element's content box (for the squarified treemap layout).
measure: function (el) { ... getBoundingClientRect() ... }
```

Yani **squarified yerleşim C#'ta hesaplanıyor, her dikdörtgen Blazor tarafından
bir DOM elementi olarak yaratılıyor**, JS sadece kutuyu ölçüyor. Buna manşet
özelliği ekleniyor: *"the tree grows live during the scan"* `[satıcı]`. Sonuç:
tarama sürerken büyüyen ağaç için sürekli .NET → WebView interop sınırından geçen
Blazor render-tree diff'i ve DOM mutasyonu.

### 3.3 Bizim tarafla karşılaştırma

| | FreeSize | spacetrace |
|---|---|---|
| Kabuk | Photino.NET + OS webview | Tauri v2 + OS webview |
| Çekirdek dil | C# / .NET (gömülü CoreCLR) | Rust |
| Numaralandırma | `GetFiles` (eager dizi) | `read_dir` + `symlink_metadata` |
| Paralellik | izi yok | rayon, **5.2×** `[ölçüm]` |
| Yerleşim | C#'ta | Rust'ta (`crates/treemap`, 21 test) |
| Çizim | Blazor → **DOM** | **Canvas2D**, yalnızca görünen dikdörtgenler |
| Tarama sırasında çizim | evet (manşet özellik) | hayır |

### 3.4 Dürüstlük sınırı

Bunlar **mimari kanıt + kendi makinemizde ölçülmüş mekanizma**, ama FreeSize'ın
kendisi ölçülmedi. Metadata'da bir API'nin yokluğu güçlü kanıt, matematiksel
kesinlik değil (obfuscation veya generic instantiation adları saklayabilir —
bu API'ler için olası değil ama imkânsız değil). Kesin sayı için ikili
çalıştırılıp `sample <pid>` ile thread sayısı ve `fs_usage` ile girdi başına
syscall ölçülmeli.

Ayrıca: FreeSize v0.3.1, Haziran 2026 güncellemeli, **hiçbir yerde tek kullanıcı
yorumu bulunamadı** `[bulunamadı]` (Reddit, HN, AlternativeTo, G2, Capterra,
Softpedia ayrı ayrı arandı). Yani hız sorunu büyük olasılıkla "olgunlaşmamış
ürün" hikâyesinin parçası, kalıcı bir mimari tercih değil — bir sonraki
sürümlerinde düzeltebilirler.

### 3.5 Satıcı ve konumlandırma

lightnet multimedia GmbH, Graben/İsviçre (UID CHE-435.553.655). Ürün dört
üründen biri. Konumlandırma "Swiss made", "no tracking".

| Kademe | Fiyat | Kapsam `[satıcı]` |
|--------|-------|-------------------|
| Free | CHF 0 | Sınırsız tarama, treemap + sunburst + heatmap |
| Pro | **CHF 29/yıl** | + arka plan izleme, **"FreeSize Portal & history"**, **"Multiple devices, centrally"** |

Portal metni birebir: *"All your devices at a glance: history, trends and usage
of your volumes — hosted in Switzerland."* Portal kayıt arkasında olduğu için
gerçek derinliği (kaba kullanım grafiği mi, gerçek snapshot diff mi)
`[bulunamadı]`. **Ama pazarlama metni bizim konumlandırmamızla doğrudan
örtüşüyor** ve barındırma İsviçre bulutunda — self-host değil.

---

## 4. Nerede iyiyiz, nerede kötüyüz

### 4.1 Gerçek üstünlükler

**① Tek üründe CLI + masaüstü + filo panosu, aynı snapshot formatıyla.**
Tablodaki hiç kimse üçünü birden yapmıyor. Teknik dayanağı: `store::load`
snapshot'ın nereden geldiğini bilmiyor (K4).

**② Açık kaynak + self-host + filo geçmişi kombinasyonu.** Bunu yapan tek diğer
ürün SpaceObServer: Windows-only, MSSQL, ~$283+. FreeSize'ın portalı İsviçre
bulutu. Homelab/selfhosted kitlesi için bu ayrım belirleyici.

**③ Ölçü dürüstlüğü — kategoride kimsenin konuşmadığı konu.** `SizeBasis`
(değişmez #6), K6 (yüzde değil boş/toplam), K7 (dayanağı zayıfsa tahmin
söylenmez). Rakiplerin hiçbirinin belgesinde bu ayrımlar `[bulunamadı]`.
Kıyasla WizTree kendi FAQ'sinde toplamının Windows'un rakamından "neredeyse her
zaman biraz az" olduğunu kabul ediyor `[satıcı]`.

**④ macOS/Linux'ta paralel yürüyüş.** ncdu 2 hâlâ tek thread. Ölçülen 5.2×.

### 4.2 Gerçek zayıflıklar

| Zayıflık | Kim daha iyi |
|----------|--------------|
| **Windows MFT hızlı yolu yok** | WizTree, TreeSize (yönetici), WinDirStat 2.5.0 |
| ~~Windows `alloc` yanlış + hardlink dedupe kapalı~~ → **9 Eylül 2026'da yazıldı, CI onayı bekliyor.** Kalan sapma: Windows'ta dizin blokları `alloc`'a girmiyor | TreeSize, WizTree, WinDirStat 2.5.0 |
| ~~APFS clone tekilleştirme yok~~ → **9 Eylül 2026'da kapandı** (`F_LOG2PHYS_EXT`, varsayılan açık) | **DaisyDisk 4.34** (clone'un ilk görünümünü sayıp kalanına 0 bayt veriyor) |
| **Bellek hedefin 11–17 katı** | dua-cli (64 B), ncdu 2 (25 B) |
| ~~Snapshot bütünlük kontrolü yok~~ → **10 Eylül 2026'da kapandı** (şema v3, içerik SHA-256'sı; import reddediyor, `spacetrace verify` denetliyor) | dua-cli (SHA-256) |
| **Artımlı yeniden tarama yok** | SpaceObServer (USN Journal) |
| **HDD/ağ için mod yok** | gdu (`--sequential`), QDirStat (inode sıralaması) |
| **Thread sayısı ayarlanmıyor** | erdtree (ampirik 3), TreeSize (CPU yüküne göre) |
| **Kod imzalama yok** | FreeSize, Diskaroo, TreeSize, WizTree — hepsi imzalı |
| **Sıfır kullanıcı, sıfır dağıtım varlığı** | hepsi |
| **Sunburst/heatmap görünümü yok** | FreeSize, Filelight |
| **JSON içe aktarma yok** (dışa aktarma var) | ncdu |

### 4.3 Eylül 2026'da kaybedilen iki farklılaştırıcı

Bunlar bu araştırmanın en önemli çıktısı, çünkü **mevcut belgelerdeki
konumlandırmayı yanlış hâle getiriyorlar.**

**① `dua-cli` v2.44.0 (30 Ağustos 2026) diff'i yakaladı.**
`dua interactive --export before.dua`, sonra `dua diff OLD NEW` — "additions,
removals, and signed size changes as a compact, colored context tree". Format
`DUASNAP\0`, zlib akış sıkıştırması, SHA-256 bütünlük. Release sayfasından
doğrulandı. **"İki zaman noktasını karşılaştır" artık tek başına
farklılaştırıcı değil**; MIT lisanslı bir CLI'da ücretsiz var. Bize kalan
**uzak ajan + filo**.

**② FreeSize Pro tam bizim cümlemizi kuruyor**, CHF 29/yıl.

[WHY.md](WHY.md)'deki şu cümle **artık yanlış**: *"Uzak makine + tarama geçmişi
yalnızca SpaceObServer'da var ve $600+/yıl; altında hiçbir şey yok."*

---

## 5. Konumlandırma sonuçları

**En kritik karar: hızı manşet yapmamak.** Hız manşeti bizi WizTree'yle Windows'ta
MFT dövüşüne sokar ve o dövüş şu an kaybediliyor (§4.2). Ayrıca §1.1'in gösterdiği
gibi "Rust olduğu için hızlı" ölçümle desteklenmiyor.

Manşet sırası:

1. **"Diskin dolmasını bekleme — neyin büyüdüğünü söyleriz."** Rakiplerin tamamı
   "şu an ne var" sorusunu cevaplıyor; bizim cevabımız "geçen haftadan beri ne
   değişti". Somut hâli: *"Sunucun 4 gün sonra dolacak ve suçlu
   `/var/log/nginx`."*
2. **"Sunucuların dâhil. Ajan açık kaynak, veriler sende kalıyor."** FreeSize'ın
   karşılığı İsviçre bulutu; SpaceObServer'ın karşılığı $283 + MSSQL + Windows.
3. **"Rakamlar doğru — ve bunu ispatlıyoruz."** `du` ile birebir eşleşmeyi
   göstermek (yan yana ekran görüntüsü) kategoride kimsenin yapmadığı bir güven
   hamlesi. Yanına K6.
4. **"Sunucuda CLI, laptop'ta treemap, ekipte pano — aynı snapshot."**

**Kullanılmaması gerekenler:** "en hızlı" (kanıtlanamıyor), "Windows'ta çalışır"
(MFT ve `alloc` düzelene kadar riskli), "ücretsiz" (K2'yi bulanıklaştırır).

---

## 6. Kaynaklar

Ölçümler bu makinede alındı ve §1'deki yöntemle tekrarlanabilir. Rakip
kaynakları:

- **FreeSize:** [freesize.ch](https://freesize.ch/en/),
  [treesize-alternative](https://freesize.ch/en/treesize-alternative.html),
  [wiztree-alternative](https://freesize.ch/en/wiztree-alternative.html),
  imprint; `get.freesize.ch/dl.php` üzerinden indirilen ikililer
- **dua-cli:** [v2.44.0 release](https://github.com/Byron/dua-cli/releases/tag/v2.44.0)
- **ncdu 2:** [dev.yorhel.nl/doc/ncdu2](https://dev.yorhel.nl/doc/ncdu2)
- **TreeSize:** [features](https://www.jam-software.com/treesize/features.shtml),
  [NTFS notları](https://manuals.jam-software.com/treesize/EN/notesonntfs.html),
  [tarama seçenekleri](https://manuals.jam-software.de/treesize/EN/scan_options.html);
  Delphi doğrulaması [Embarcadero blog](https://blogs.embarcadero.com/powerful-file-and-disk-space-manager-software-for-windows-is-built-in-delphi/)
- **WizTree:** [about](https://www.diskanalyzer.com/about), [faq](https://diskanalyzer.com/faq)
- **WinDirStat:** [discussions/218](https://github.com/windirstat/windirstat/discussions/218) (maintainer açıklaması), GitHub release API
- **SpaceObServer:** [ajan](https://www.jam-software.com/spaceobserver/spaceobserveragent.shtml),
  [tarama seçenekleri](https://manuals.jam-software.com/spaceobserver/EN/scan_options.html)
- **DaisyDisk APFS clone:** [4.34 sürüm notu](https://daisydiskapp.com/blog/daisydisk-4-34-released)
- **QDirStat:** [DirReadJob.cpp](https://github.com/shundhammer/qdirstat/blob/master/src/DirReadJob.cpp)
- **gdu:** [README](https://github.com/dundee/gdu/blob/master/README.md)
- **dut:** [codeberg](https://codeberg.org/201984/dut)
- **erdtree:** [CHANGELOG](https://github.com/solidiquis/erdtree/blob/master/CHANGELOG.md)
- **btdu:** [README](https://github.com/CyberShadow/btdu/blob/master/README.md)
- **Diskaroo:** [bravely.dev/diskaroo](https://bravely.dev/diskaroo), vs/wiztree ve vs/treesize karşılaştırma sayfaları
- **DiskRaptor:** [github.com/SunMe1977/DiskRaptor](https://github.com/SunMe1977/DiskRaptor)
