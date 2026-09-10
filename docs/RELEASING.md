# Sürüm ve dağıtım

Üç bileşenin (CLI + ajan, masaüstü, hub) nasıl derlenip nasıl indirilebilir hâle
geldiği. Bu belge gerekçe belgesi olduğu için Türkçe; üretilen her şey — sürüm
notları, indirme sayfası, kurulum talimatları — İngilizce (K1).

## Neden hepsi bu depoda yayınlanıyor

Bu düzen, masaüstü ve hub private'ken kurulmuştu: private bir deponun release
varlıkları kimlik doğrulaması olmadan indirilemiyor, o yüzden varlıkların public
bir depoda durması zorunluydu.

**O kısıt artık yok** — üç depo da public. Ama düzen duruyor, çünkü artık başka
sebepleri var:

- İndirme sayfası **tek bir GitHub API çağrısıyla** üç bileşenin sürümünü
  öğreniyor. Üç ayrı depoya dağıtmak üç çağrı ve üç hata yolu demek.
- `install.sh` ve sitenin `src/data/releases.ts` dosyası tek bir depoya bağlı.
  Dağıtmak, çalışan her indirme bağlantısını hiçbir kazanç karşılığında
  yeniden yazmak olurdu.

Yani her depo kendi kodunu kendi CI'ında derler, çıktıyı **bu deponun
release'lerine** yayınlar.

## Kanallar

Etiket adları sabit. İndirme sayfası bunlara doğrudan bağlanıyor, yani
**bunları yeniden adlandırmak siteyi kırar.**

| Etiket | İçerik | Tetikleyen |
|--------|--------|------------|
| `continuous` | CLI + ajan, `main`'in son hâli | bu depoya push |
| `v*` | CLI + ajan, kararlı | bu depoda `v*` etiketi |
| `desktop-continuous` | Masaüstü kurulumları | desktop deposuna push |
| `desktop-v*` | Masaüstü, kararlı | desktop deposunda `v*` etiketi |
| `hub-continuous` | Hub ikilileri | hub deposuna push |
| `hub-v*` | Hub, kararlı | hub deposunda `v*` etiketi |

`continuous` release'leri **silinip yeniden oluşturulur**, düzenlenmez: böylece
etiket `main`'i takip eder ve matristen çıkarılan bir varlık indirme sayfasında
ölü bağlantı olarak kalmaz. Kısa bir 404 penceresi var; sürekli kanal için kabul
edilebilir.

Varlık adları sürümü taşıyor (`spacetrace-continuous-x86_64-apple-darwin.tar.gz`)
çünkü `install.sh` dosya adını verilen sürümden kuruyor. Yani
`SPACETRACE_VERSION=continuous` hiçbir değişiklik olmadan çalışıyor.

## Konteyner imajları

| İmaj | Ne | Etiketler |
|------|-----|-----------|
| `ghcr.io/unalcakir28/spacetrace` | ajan + CLI | `main`, `edge`, `v*`, `latest` |
| `ghcr.io/unalcakir28/spacetrace-hub` | hub | `main`, `edge`, `v*`, `latest` |

İkisi de amd64 + arm64. **Kaynaktan değil, önceden derlenmiş musl ikililerinden**
kuruluyor (`.github/docker/Dockerfile.release`): amd64 bir runner'da QEMU altında
Rust derlemek onlarca dakika sürüyor ve düzenli olarak belleği tüketiyor. Depo
kökündeki `Dockerfile` kaynaktan derlemeye devam ediyor — `docker build .` bir
klonda çalışsın diye.

## Yayına alırken gereken elle adımlar

Bunlar bir kez yapılıyor ve otomatikleştirilemiyor.

### 1. `RELEASE_TOKEN` (masaüstü ve hub depoları için)

Bir depodaki `GITHUB_TOKEN` **başka** bir depoya yazamaz — bu, depo public olsa
da geçerli; mesele gizlilik değil, token'ın kapsamı. Masaüstü ve hub kendi
çıktılarını buraya yayınladığı için ayrı bir token gerekiyor:

1. <https://github.com/settings/personal-access-tokens/new>
2. Repository access → **Only select repositories** → `unalcakir28/spacetrace`
3. Permissions → Repository permissions → **Contents: Read and write**
4. Süreyi seçip oluştur, tokenı kopyala

Sonra iki depoya da sır olarak ekle:

```bash
gh secret set RELEASE_TOKEN --repo unalcakir28/spacetrace-desktop
gh secret set RELEASE_TOKEN --repo unalcakir28/spacetrace-hub
```

Sır yoksa iş akışı **hata vermiyor**: varlıkları kendi deposunda yayınlıyor ve
bir uyarı basıyor. Yani ilk push'lar boşa gitmez — ama varlıklar sitenin
beklediği yerde olmaz, yani indirme bağlantıları 404 verir.

Token süresi dolduğunda yayınlama adımı 403 ile düşer. Yenile ve aynı komutu
tekrar çalıştır.

### 2. GHCR paket görünürlüğü

GHCR paketleri, deposu public olsa bile **private başlıyor** — paket
görünürlüğü depo görünürlüğünden ayrı. `docker pull` kimlik doğrulaması
istemesin diye her yeni paket için bir kez:

Paket sayfası → **Package settings** → Change visibility → **Public**.

- <https://github.com/users/unalcakir28/packages/container/spacetrace/settings>
- <https://github.com/users/unalcakir28/packages/container/spacetrace-hub/settings>

`spacetrace` imajı bu public depodan geldiği için zaten public olabilir; yine de
ilk yayından sonra kontrol et.

### 3. GitHub Pages — **artık bu depoda değil**

Site kendi deposuna taşındı, yani bu adım oraya ait. Kaydı burada tutmakta bir
fayda var, çünkü aynı tuzağa iki kez düşülüyor: `GITHUB_TOKEN`'ın hiç var
olmamış bir Pages sitesini oluşturma izni yok ("Resource not accessible by
integration"), yani `configure-pages` bunu kendisi başlatamıyor. Yeni bir Pages
sitesi bir kez elle açılmak zorunda:

```bash
gh api -X POST repos/unalcakir28/spacetrace-website/pages -f build_type=workflow
gh api -X PUT  repos/unalcakir28/spacetrace-website/pages -f cname=spacetrace.teknobakkall.com
```

Alan adı Cloudflare'de CNAME olarak `unalcakir28.github.io`'ya bakıyor ve
**proxy kapalı** (gri bulut) olmak zorunda: turuncu bulutla önde dururken
GitHub alan adını doğrulayamıyor ve Let's Encrypt sertifikasını üretemiyor.

## Kararlı sürüm kesmek

Her sürümde önce changelog, sonra sürüm numarası, sonra etiket.

**1. Changelog'u kapat.** `promote`, o bileşenin `unreleased` girdilerini yeni
bir sürüme taşır ve `changelog.json`'ı yeniden yazar. Boş bir `unreleased`
reddedilir: notu hiçbir şey söylemeyen bir sürüm, hiç kesilmemiş bir sürümden
kötüdür — okuyucu notların mı eksik olduğunu yoksa sürümün mü boş geçtiğini
ayırt edemez.

```bash
cargo run -p spacetrace-changelog -- promote --component cli --version 0.2.0
cargo run -p spacetrace-changelog -- markdown --component cli > CHANGELOG.md
```

**2. Sürüm numarasını yükselt ve etiketle.**

```bash
# CLI + ajan (bu depo): Cargo.toml içindeki workspace.package.version
git tag v0.2.0 && git push origin v0.2.0

# masaüstü: package.json, src-tauri/Cargo.toml ve src-tauri/tauri.conf.json
git tag v0.2.0 && git push origin v0.2.0

# hub: Cargo.toml
git tag v0.2.0 && git push origin v0.2.0
```

**Masaüstü ve hub için sıra bozulamaz.** Girdileri bu depoda duruyor (K11), ve
ikili changelog'u kendi `Cargo.lock`'unun pinlediği çekirdek sürümünden gömüyor:

1. girdiyi burada commit'le ve push et
2. diğer depoda `Cargo.lock`'taki çekirdek pin'ini ilerlet
3. orada etiketle

İkinci adım atlanırsa uygulama, kendi yayın notlarından eski bir changelog
gömerek çıkar — kullanıcı "Yenilikler" penceresini açtığında az önce kurduğu
sürümü orada bulamaz.

Etiket `v*` olduğu sürece iş akışı kararlı kanala yayınlıyor. Masaüstü ve hub
etiketleri bu depoda `desktop-v0.2.0` / `hub-v0.2.0` olarak görünür — aynı isim
alanında üç bileşen olduğu için.

Kuru çalışma: `workflow_dispatch` ile bir etiket adı ver. Derleme yapılır,
yayınlama adımı atlanır — `publish` kutusunu işaretlemezsen.

`publish: true` ile elle yayınlama, `paths-ignore` filtresinin bir etiket
push'unu atladığı durumun çıkış yolu (aşağıya bak). Etiket hedef depoda
default branch'in ucunda oluşturulur.

### "Latest" CLI'nındır, sırayla belirlenmez

GitHub'ın `releases/latest` uç noktası **en son yayınlanan** sürümü döndürüyor,
bileşen ayırmadan. Üç bileşenin indirmeleri bu depoda olduğu için bu, bir
masaüstü ya da hub sürümü kesmenin CLI'ın yerini alması demek.

Zararı somut: `spacetrace update` ve ajan, **v0.4.0'a kadarki sürümlerde** o uç
noktayı okuyor, ve `desktop-v0.4.0` gibi bir etiket onların ayrıştırıcısında
hiçbir sürüme karşılık gelmiyor — güncelleme kontrolü sessizce kapanıyor ve
öyle kalıyor. 9 Eylül 2026'da `hub-v0.3.0` bu yeri alınca ölçüldü.

O yüzden masaüstü ve hub iş akışları kararlı sürümü **`gh release create
--latest=false`** ile yayınlıyor. Yeni bir bileşen deposu eklenirse aynısını
yapması gerekiyor. Bir şekilde yer kaptırılırsa geri alma tek komut:

```bash
gh release edit <cli etiketi> --repo unalcakir28/spacetrace --latest
```

v0.4.1 ve sonrası etiket önekine bakıyor (`is_ours`), yani bu tuzaktan
etkilenmiyor — kural, hâlâ eski ikiliyi çalıştıranlar için duruyor.

## Hangi commit neyi tetikliyor

Beş hedefli bir derleme bedava değil, bu yüzden yalnızca kodu ilgilendiren
değişiklikler sürüm iş akışını başlatıyor:

- `docs/**`, `website/**`, `*.md`, `tasks/**` → sürüm iş akışı **çalışmaz**
- `website/**` → Pages iş akışı çalışır (ve yalnızca o)

Site ile ikili derlemesi bilinçli olarak ayrı: indirme sayfasındaki bir yazım
hatasının düzeltilmesi beş platformluk bir derlemeyi beklememeli, ve bir derleme
hatası bir belge düzeltmesinin yayına girmesini engellememeli.

**Dikkat:** GitHub'da yol filtreleri `on.push` içinde daldan bağımsız, yani
etiket push'larına da uygulanıyor. Yalnızca `docs/` veya `*.md` değiştiren bir
commit'i etiketlersen sürüm iş akışı hiç çalışmaz. Pratikte sorun değil —
sürüm kesmek `Cargo.toml` / `package.json` / `tauri.conf.json` değiştirmeyi
gerektiriyor ve bunların hiçbiri yoksayılmıyor. Yine de olursa çıkış yolu
`workflow_dispatch` + `publish: true`.

## Kod imzalama — durum ve çıkış planı

**Hiçbir şey imzalı değil.** macOS'ta bundle yalnızca linker'ın arm64 için
zorunlu attığı ad-hoc imzayı taşıyor (`codesign` → `Signature=adhoc`,
`TeamIdentifier=not set`, `spctl` → `rejected, no usable signature`); Windows
kurulumu tamamen imzasız.

Bunun bedeli 10 Eylül 2026'da ölçüldü: macOS 26.5.2'de indirilen `.dmg`
açılmıyor, ve **Apple'ın "sağ tık → Aç" kısayolunu macOS 15'te kaldırmış
olması** yüzünden sitede yıllardır yazan talimat geçersizdi. Geriye Sistem
Ayarları → Gizlilik ve Güvenlik → Yine de Aç kalıyor, ve o düğmenin ad-hoc
imzalı bir uygulamada göründüğü **doğrulanmadı**.

Geçici çözüm olarak `install-desktop.sh` ve `install-desktop.ps1` eklendi.
Bunlar Gatekeeper'ı kandırmıyor: ilk açılış kontrolünü tetikleyen
`com.apple.quarantine` özniteliğini (Windows'ta Mark-of-the-Web) **tarayıcı**
yazıyor, curl ve `Invoke-WebRequest` yazmıyor. Ölçüldü — curl ile inen dmg'de
hiç genişletilmiş öznitelik yok, içinden çıkan uygulama tek diyalog görmeden
açılıyor. Karşılığında imzanın verdiği *kimlik* garantisi yerine yalnızca
SHA256SUMS'ın verdiği *bütünlük* garantisi kalıyor; betikler bunu açıkça yazıyor.

Homebrew bu boşluğu dolduramaz: `--no-quarantine` Homebrew 4.7'de kaldırıldı ve
Gatekeeper'dan geçemeyen cask'lar **1 Eylül 2026'da** kendi tap'inizde bile
desteklenmez oldu.

Maliyet, karar verilirse:

| | Ücret | Sonuç |
|---|---|---|
| Apple Developer Program | $99/yıl | Notarize edilmiş `.dmg`, macOS uyarısı tamamen kalkar |
| Windows OV sertifikası | ~$220–400/yıl | SmartScreen anında temizlenmez, itibar birikir |
| Windows EV sertifikası | ~$500–660/yıl | SmartScreen ilk günden temiz |

Azure Artifact Signing ($9.99/ay) **Türkiye'ye kapalı** — ABD, Kanada, AB ve
İngiltere ile sınırlı, yani Windows için ucuz yol yok.

### İmzasız ≠ kimliksiz: TCC ayrı bir sorun ve o çözüldü

Gatekeeper ile TCC (izinler) aynı şey değil, ve ikincisi **para istemiyordu**.

macOS, Tam Disk Erişimi'ni ve klasör izinlerini uygulamanın *designated
requirement*'ına bağlıyor. Ad-hoc imzanın böyle bir şeyi yok, o yüzden sistem
ikilinin cdhash'ine düşüyor — ve cdhash her derlemede değişiyor. Sonuç: her
sürüm macOS için başka bir uygulama, kullanıcının verdiği izin Sistem
Ayarları'nda **açık görünüyor ama uygulanmıyor**, ve her güncellemeden sonra
tarama klasör klasör yeniden soruyor. 10 Eylül 2026'da kullanıcı bildirdi,
anahtar açıkken.

Çözüm kendinden imzalı bir sertifika. Gatekeeper'a hiçbir faydası yok, ama
kimliği sabitliyor. Ölçüldü: içerikleri farklı iki paket (cdhash `53007fdd…` /
`de0072e5…`) tek bir requirement paylaşıyor:

```text
identifier "com.spacetrace.desktop" and certificate root = H"940f909c…"
```

`root`, `leaf` değil: codesign zinciri nasıl görüyorsa onu yazıyor ve sertifika
derleme makinesinde güvenilir kök olduğu için `root` çıkıyor. Aynı sertifika
güvenilmezken `leaf` yazıyordu — ikisi de derlemeler arası sabit, ama **farklı
dizeler**, ve TCC requirement'ı yazıldığı gibi karşılaştırıyor. Yani güven
adımını kaldıran bir derleme geçerli bir imza üretir ve yine de herkesin iznini
düşürür. Sürüm iş akışı bu yüzden tam olarak `root` biçimini doğruluyor.

Kurulumdaki üç tuzak, üçü de yaşandı:

- **OpenSSL 3 varsayılan p12'yi macOS okuyamıyor.** SHA-256 MAC yazıyor;
  `security import` "MAC verification failed (wrong password?)" diyor ve sizi
  şifreye baktırıyor. `openssl pkcs12 -export -legacy` gerekiyor.
- **Sertifika derleme makinesinde güvenilir olmalı.** Tauri kimliği
  `security find-identity -v` ile arıyor, o da yalnızca geçerli kimlikleri
  listeliyor; kendinden imzalı bir sertifika trustRoot yapılmadan geçerli
  sayılmıyor. Runner'lar tek kullanımlık, yani orada güvenmek başka hiçbir
  makineye ulaşmıyor.
- **Sertifikayı değiştirmek herkesin iznini sıfırlar.** Requirement leaf
  parmak iziyle yazılı. Süresi 2036'da doluyor; yenilemek yeni bir sertifika
  demek, yani kullanıcılar izni bir kez daha verecek.

Sır: `APPLE_CERTIFICATE` (p12'nin base64'ü) ve `APPLE_CERTIFICATE_PASSWORD`,
masaüstü deposunda. Özel anahtar `~/.spacetrace/macos-signing.p12`, hiçbir
deponun içinde değil. Sır yoksa derleme ad-hoc'a düşüyor ve uyarı basıyor;
sürüm iş akışı ayrıca paketin requirement'ını doğruluyor, yani sessizce geri
düşmüyor.

**İmzalama geldiğinde yapılacaklar** (bu bölümün varlık sebebi bu liste):

1. `install-desktop.sh` ve `install-desktop.ps1` **silinir** — bakımı yapılacak
   dosyalar değil, bir eksiğin yaması.
2. Site deposunda `installAltTitle` / `installAltBody` / `installAltNote`
   anahtarları ve `Download.astro`'daki alternatif kutusu kaldırılır.
3. `installMacBody` "çift tıkla, açılır" hâline döner.
4. Masaüstü `release.yml`'deki "Not code-signed" sürüm notu paragrafı silinir.

## Site

Site artık bu depoda değil:
**[unalcakir28/spacetrace-website](https://github.com/unalcakir28/spacetrace-website)**
— Astro, beş dil, 31 sayfa, `spacetrace.teknobakkall.com` adresinden yayında.
Nasıl çalıştığı ve kolay bozulan yerleri o deponun `CLAUDE.md`'sinde.

Buradan ayrıldı çünkü onu burada tutan tek şey adresti: GitHub Pages proje
sitesini `/<depo-adı>/` altında sunuyor, yani depo adı URL'in parçasıydı ve
ayırmak adresi bozardı. Kendi alan adı o bağı kopardı; sitenin `crates/` ile
zaten hiçbir kod bağı yoktu.

**Bu depoda kalan tek bağ, aşağıdaki indirme sözleşmesi.** Sitenin
`src/data/releases.ts` dosyası buradaki üç sürüm iş akışının ürettiği etiket ve
varlık adlarına birebir bağlı. Yukarıdaki tabloda bir ad değiştirirsen site
deposunu da aynı gün güncelle, yoksa her indirme bağlantısı sessizce kırılır —
ve artık iki ayrı depo olduğu için tek bir CI adımı bunu yakalamıyor.
