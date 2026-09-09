# Araştırma özeti (Eylül 2026)

Projenin kararları 6 Eylül 2026'da yapılan pazar ve teknik araştırmaya dayanıyor.
Bu belge o araştırmanın **karar veren** kısmını depoda tutar; ürün gerekçesi için
[WHY.md](WHY.md), tasarım için [ARCHITECTURE.md](ARCHITECTURE.md).

**9 Eylül 2026'da revize edildi.** Rakiplere odaklı, kendi makinemizde alınmış
ölçümlerle desteklenen ve her iddiası kanıt etiketli devamı
[COMPETITORS.md](COMPETITORS.md) içinde. Aşağıda o incelemenin **düzelttiği**
yerler açıkça işaretli — eski hâli silinmedi, çünkü hangi kararın hangi bilgiyle
verildiği kaybolursa kararı yeniden değerlendirmek imkânsızlaşıyor.

> Fiyatlar, sürümler ve mağaza politikaları hızla değişir. Bir karar bunlardan
> birine dayanıyorsa kullanmadan önce kaynağı yeniden kontrol et.

## 1. Rakipler ve fiyat çıpaları

| Ürün | Platform | Fiyat | Not |
|------|----------|-------|-----|
| TreeSize Free / Personal / Pro | Windows | $0 / $50 kalıcı / $49.20 yıl | Pro'da SSH-UNC-bulut tarama; **kalıcı lisans yalnızca Personal'da kaldı**. Düzeltme: **Free bile yönetici olarak koşarsa MFT okuyor** |
| SpaceObServer | Windows + sunucu | $600/instance/yıl + kullanıcı | Veritabanı destekli, kurumsal. **USN Journal ile artımlı tarama** — en yakın mimari rakip |
| WizTree | Windows | Kişisel $0, iş $25–1.800 | NTFS MFT okur; uzak tarama **yok** |
| WinDirStat 2.x | Windows | GPL | Düzeltme: 2024'teki hız kazancı **sürücü başına çok thread**'di; MFT ancak **v2.5.0'da (Ocak 2026)** ve opsiyonel |
| DaisyDisk | macOS | $9.99 kalıcı, 5 Mac | Mac'te duygusal fiyat çıpası. **APFS clone'larını tekilleştiriyor** (4.34) — bizde yok |
| FreeSize | Win/mac/Linux | $0, Pro CHF 29/yıl | **.NET + Photino.Blazor** (ikili incelendi); Pro'da "Portal & history, multiple devices centrally" |
| Diskaroo | Win/mac/Linux | $19.99 kalıcı | Ayrı native kod tabanları (Swift + WPF) |
| DiskRaptor | Win/mac/Linux | MIT | Rust + Tauri |
| **dua-cli** | çapraz | MIT | **v2.44.0 (30 Ağu 2026): `--export` + `dua diff`** — geçmiş karşılaştırması artık ücretsiz bir CLI'da |
| ncdu 2 / gdu / dust | Linux/TUI | ücretsiz | Sunucularda fiili standart |

**Karar için önemli olan iki nokta.**

**(a) Masaüstü treemap kategorisi 2025–26'da üç yeni ürünle doldu, biri
ücretsiz** — oraya dördüncü olarak girmek savunulamaz. Bu hâlâ geçerli.

**(b)** İlk hâli şöyleydi: *"Uzak makine + tarama geçmişi yalnızca
SpaceObServer'da var ve $600+/yıl; altında hiçbir şey yok."* **Bu cümle 9 Eylül
2026'da yanlışlandı** — aynı ay içinde iki gelişme oldu
([COMPETITORS.md §4.3](COMPETITORS.md)):

- `dua-cli` v2.44.0 (30 Ağustos 2026) `--export` + `dua diff` ile geçmiş
  karşılaştırmasını MIT lisansı altında ücretsiz verdi.
- FreeSize Pro (CHF 29/yıl) *"tüm cihazlarınız tek bakışta: geçmiş, trendler"*
  diyen bir portal açtı.

Yani boşluk "hiç kimse yok" değil, **"açık kaynak + self-host + filo geçmişi bir
arada yok"**. Kalan farklılaştırıcı bu üçlünün kesişimi; tek başına "iki zaman
noktasını karşılaştır" değil. Fiyat hipotezi de bu daralmayı hesaba katmak
zorunda ([WHY.md](WHY.md) → Konumlandırma).

**Lisans penceresi:** JAM Software 2025'te TreeSize'ı aboneliğe çevirdi, Temmuz
2026'da kalıcı lisans sahiplerine güncelleme vermeyi kesti. Tepki büyük oldu;
kalıcı lisans seçeneği sunmak kendi başına bir edinim argümanı.

## 2. Masaüstü framework karşılaştırması

Mobil kapsamdan çıktıktan sonra (bkz. §5) sıralama:

| Framework | Win/mac/Linux | Kurulum boyutu | 100k dikdörtgen | Not |
|-----------|---------------|----------------|-----------------|-----|
| **Tauri v2 + Rust** | 5 / 5 / 4 | **3–15 MB** | Canvas/WebGL, WebView farkları | Rust'ı süreç içinde çağırır, FFI köprüsü yok. **Seçilen.** |
| Avalonia 12 | 5 / 5 / 5 | ~orta (NativeAOT) | Skia, piksel-aynı | Uçtan uca C#; **yedek plan** |
| Flutter + FRB | 5 / 5 / 5 | 20–80 MB | Impeller, batched canvas | Kozu "beş platform tek kod"du; mobil çıkınca gerekçesi kalmadı |
| Qt/QML | 5 / 5 / 5 | orta | En iyi | Lisans $618+/yıl, C++ ekibi yok |
| Electron | 5 / 5 / 5 | 50–150 MB | orta | Tauri aynı UI'ı 10× küçük paketle veriyor |
| .NET MAUI | 4 / 3 / **1** | — | — | Linux resmen desteklenmiyor |

Tauri'nin bilinen riski Linux'taki **WebKitGTK**: treemap üç WebView'da da test
edilmeli (Faz 3 çıkış kriteri).

## 3. Hızlı tarama teknikleri (Faz 3 için)

Şu an tek taşınabilir arka uç var (`read_dir` + `symlink_metadata`). Hız
beklentisi MFT okuyan WizTree ile belirlenmiş durumda; Windows'ta hızlı yol
olmadan çıkmak kaybetmek demek.

### Windows
- **MFT doğrudan okuma:** NTFS Master File Table'ı diskten ham okur, OS'i atlar.
  Kullanıcı raporu: WinDirStat 18 dk → WizTree ~14 s. Yönetici hakkı gerekir;
  ReFS'te MFT yok; ağ/FAT sürücülerde normal enumerasyona düşülür.
  Crate: [`usn-journal-rs`](https://github.com/wangfu91/usn-journal-rs)
  (MFT enumerasyonu + USN change journal ile **artımlı** yeniden tarama),
  [`ntfs-reader`](https://lib.rs/crates/ntfs-reader).
- **Yetkisiz yol:** `NtQueryDirectoryFileEx`, 64 KB+ buffer,
  `FileIdBothDirectoryInformation` — boyut ve file id tek çağrıda gelir, hardlink
  dedupe için dosya başına ek syscall gerekmez.
  `FindFirstFileEx` + `FIND_FIRST_EX_LARGE_FETCH` ölçümde duvar saatini ~2×
  düşürüyor ([ölçüm](https://blog.s-schoener.com/2024-06-09-find-first-large-fetch/)).

### macOS
- `getattrlistbulk`: boyut/tarih gerektiğinde `readdir + lstat`'tan belirgin
  hızlı. `dumac` bununla `du`'dan 6.39× hızlı
  ([ölçüm](https://healeycodes.com/maybe-the-fastest-disk-usage-program-on-macos)).
  Crate: [`getattrlistbulk-rs`](https://github.com/quivent/getattrlistbulk-rs).
- **APFS tuzağı:** clone'lar blok paylaşır, Finder onlarca GB sapabilir.
  DaisyDisk clone'un yalnızca ilk görünümünü sayıyor. Snapshot'lar taramayla
  görünmez; `tmutil listlocalsnapshots` gerekir.
- **Mac App Store'a girme.** Sandbox'lı uygulamaya Full Disk Access verilse bile
  App Sandbox denetimlerini aşmıyor (Apple DTS). DaisyDisk'in MAS sürümünde
  "yönetici olarak tara" yok. Developer ID + notarization ile mağaza dışından
  dağıt.

### Linux
- `getdents64` + `statx`, thread başına DFS. `dut` sıcak önbellekte `du`'dan
  6.87×, dust/dua/gdu'dan 2.8–3.75× hızlı ([dut](https://codeberg.org/201984/dut)).
- io_uring'de `getdents` yok; yalnızca toplu `statx` için işe yarar.
- **btrfs/ZFS:** snapshot, reflink, sıkıştırma ve dedup ağaç yürüyüşünü yanlış
  kılar. Doğrusu için [`btdu`](https://github.com/CyberShadow/btdu) gibi örnekleme
  gerekir.

### Genel
SSD'de work-stealing paralel DFS, HDD/ağda 1–2 thread. Bellek hedefi ncdu 2'nin
mertebesi: **~25 B/dosya** (3.8M dosya = 162 MB).

**Ölçüm (9 Eylül 2026), iki taraf da hedefin dışında:** bellekte gerçek tepe
**276–437 B/girdi**, yani hedefin 11–17 katı (10M dosyaya ekstrapole ~2,8 GB).
**Ama ~25 B hedefi de yanlış konmuş:** ncdu 2 düğüm başına `own_size`,
`own_alloc`, `files`, `dirs` tutmuyor, biz tutuyoruz — en agresif daraltmayla
taban ~93 B/girdi. Gerçekçi hedef dua-cli'nin **64 B**'ı.
Paralellik tarafı tutuyor — 1 → 8 thread arası **5.2×** — ama **16 thread'te
gerileme var**, yani "HDD/ağda 1–2 thread" kuralının yanına "SSD'de de çekirdek
sayısı kadar değil" yazmak gerekiyor. Ayrıntı ve yöntem
[COMPETITORS.md §1](COMPETITORS.md).

## 4. Treemap render (Faz 3)

- **Squarified treemap** (Bruls, Huizing, van Wijk 2000): çocukları azalan
  sırala, satıra eklerken en kötü en-boy oranı iyileşiyorsa devam et, aksi hâlde
  satırı sabitle ve kalanla yinele. [PDF](https://vanwijk.win.tue.nl/stm.pdf).
  ~300 satırlık Rust ile kendin yaz; chart kütüphanesine bağlanma.
- **Cushion treemap** (van Wijk 1999, SequoiaView → WinDirStat): dikdörtgen başına
  parabolik tümsek, hiyerarşi boyunca biriken yüzey katsayıları, sabit ışık
  vektörü. [PDF](https://vanwijk.win.tue.nl/ctm.pdf). GPU'da fragment shader'a
  birebir oturur; ucuz yaklaşım radyal gradient overlay.
- **100k+ dikdörtgen için:** layout'u zoom başına bir kez hesapla (Rust, düz
  dizi) → **LOD**: ~4–6 px²'den küçükleri bölmeyi bırak (gerçek ağaçta herhangi
  bir zoom'da görünür dikdörtgen binlerle sınırlı) → quadtree ile **culling** →
  **toplu çizim** (WebGL instanced quad). Hit-test'i widget ağacıyla değil aynı
  uzamsal indeksle yap.

## 5. Mobil neden kapsam dışı

- **iOS:** uygulama yalnızca kendi sandbox'ını görür; başka yerler Files'tan
  seçilen klasörün alt ağacıyla sınırlı (security-scoped bookmark). Ayarlar →
  iPhone Depolama'daki uygulama başına hesap için public API yok.
- **Android:** tam tarama `MANAGE_EXTERNAL_STORAGE` ister;
  [Play politikasının](https://support.google.com/googleplay/android-developer/answer/10467955)
  izinli listesinde (dosya yöneticisi, yedekleme, antivirüs, belge yönetimi,
  arama, şifreleme, cihaz taşıma) **disk analizörü yok**. İzin alınsa bile
  `Android/data` ve `Android/obb` kapalı. DiskUsage tam bu yüzden Play'den kalktı.

## 6. Dağıtım ve imzalama maliyetleri

| Kanal | Maliyet | Not |
|-------|---------|-----|
| Apple Developer Program | $99/yıl | Notarization zorunlu; MAS'a girme (§3) |
| Windows: Azure Artifact Signing | $9.99/ay | **Türkiye'den bireysel alınamıyor** (ABD/Kanada bireyler, ABD/CA/AB/UK kuruluşlar) |
| Windows: OV sertifika | $150–300/yıl | Gerçekçi yol. EV artık SmartScreen'i atlamıyor |
| Microsoft Store | Birey $0 | MSIX'i Microsoft yeniden imzalar: SmartScreen uyarısı yok. MFT için `runFullTrust` gerekir |
| Linux: AppImage / AUR / deb / rpm | $0 | Kısıtsız, disk analizörüne uygun |
| Linux: Flatpak / Snap | $0 | Sandbox sorun: Filelight Flatpak'i "işe yaramaz" raporlandı. `--filesystem=host` + gerekçe gerekir |

## 7. İncelenmeye değer projeler

Kod okurken referans: [dua-cli](https://github.com/Byron/dua-cli) (jwalk, TUI;
ayrıca **64 B arena düğümü + paylaşılan ad deposu** ve SHA-256 bütünlüklü
`DUASNAP` snapshot formatı — TODO'daki B1 ve A5 için hazır referans),
[dut](https://codeberg.org/201984/dut) (en hızlı Linux walker),
[gdu](https://github.com/dundee/gdu) (Go, JSON export),
[ncdu 2](https://dev.yorhel.nl/doc/ncdu2) (bellek modeli),
[QDirStat](https://github.com/shundhammer/qdirstat) (C++ treemap referansı),
[Czkawka](https://github.com/qarmin/czkawka) (tek çekirdek + çok arayüz deseni,
duplicate hattı),
[SquirrelDisk](https://github.com/adileo/squirreldisk) (Tauri; ölü ama okunabilir).
