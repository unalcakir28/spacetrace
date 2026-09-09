# Kararlar

Fazlar arası, geri dönüşü pahalı kararlar ve gerekçeleri. Bir karar burada
yazılıysa tartışma kapanmıştır; yeniden açmak için yeni bir bilgi gerekir.

Ürün gerekçesi [WHY.md](WHY.md), tasarım [ARCHITECTURE.md](ARCHITECTURE.md),
plan [ROADMAP.md](ROADMAP.md).

---

## K1 — Arayüz dili İngilizce · 7 Eylül 2026

CLI dizeleri, `--help` metinleri, hata mesajları, README ve ARCHITECTURE
İngilizce. WHY / ROADMAP / TODO / RESEARCH / DECISIONS ve CLAUDE.md Türkçe kalır.

**Neden.** Hedef kanallar (HN, r/selfhosted) ve hedef kullanıcı İngilizce. Geçiş
maliyeti yalnızca büyür: bu karar verildiğinde çevrilecek yüzey iki dosyada 71
satırdı; ajanın HTTP hata mesajları, TOML yorumları, systemd unit ve kurulum
betiği eklendikten sonra birkaç katına çıkacaktı. Depo henüz yayımlanmamıştı,
yani kırılacak dış bağlantı yoktu.

**Sonuç.** i18n çerçevesi eklenmez; dizeler İngilizce ve koda gömülü kalır.
"i18n yok" bir teknik borç değil, kapsam dışı ilan edilmiş bir karardır.

---

## K2 — Çekirdek ve ajan Apache-2.0, masaüstü ayrı depo · 7 Eylül 2026

| Bileşen | Lisans | Depo |
|---------|--------|------|
| `scan-core`, `store`, `diff`, `cli`, `agent` | Apache-2.0 | bu monorepo, public |
| Masaüstü (Faz 3) | ticari, Faz 3'te netleşecek | ayrı depo |
| Merkez servis (Faz 4) | ticari / kaynak-açık | ayrı depo |

**Neden.** Ajan kullanıcının kendi sunucusunda root'a yakın yetkiyle çalışıyor.
[WHY.md](WHY.md)'deki "ajan güven kazanamaz" kaybetme senaryosunun tek panzehiri
kodun okunabilir olması. Ajanı kapatmak, ürünün savunulabilir kısmını korumaz —
yalnızca kurulum oranını düşürür.

**Bunun zorladığı düzeltme.** WHY.md'nin ilk fiyat hipotezinde Pro kademesi
"sınırsız ajan" idi. Ajan açık kaynaksa bu sınır uygulanamaz: herkes derleyip
istediği kadar çalıştırır. Pro kademesi **masaüstü + uzak kaynaklar + zaman
çizelgesi** olarak yeniden tanımlandı; para, kullanıcının kolayca yeniden
yazamayacağı ve ikili olarak dağıtılan şeyden gelir.

---

## K3 — Ajan protokolü HTTP + JSON · 7 Eylül 2026

Metadata uçları JSON, snapshot gövdesi `application/octet-stream`. Sunucu
çatısı **axum**.

**Neden gRPC değil.**

- Hedef kullanıcı homelab geliştiricisi: ajanı Caddy/Traefik arkasına koyar,
  `curl` ile bakar, tarayıcıdan `/health` açar. gRPC'yi ters vekil arkasına almak
  (h2c) ve sonra tarayıcıdan tüketmek (grpc-web) bu kitleye sürtünme.
- Faz 3 masaüstü bir WebView, Faz 4 zaten axum. JSON'u üçü de doğrudan tüketir.
- Bağımlılık bütçesi: `tonic + prost` + derleme zamanı `protoc`, `axum + serde`
  yanında ağır. serde zaten workspace'te.
- gRPC'nin iki kozu burada gerekmiyor: uç sayısı ~6, snapshot transferi tek büyük
  blob ve düz HTTP gövdesi bunu zaten akıtıyor.

**Kabul edilen maliyet — ölçüldü.** Faz 1'de workspace 26 crate'ti; axum, tokio,
reqwest ve zstd sonrası **162**. Bu, CLAUDE.md'nin bağımlılık cimriliği kuralına
göre büyük bir sıçrama ve karar verilirken tahmin edilen (~60) rakamın çok
üstünde. Yine de kabul edildi: tek HTTP yığını hem ajanda hem Faz 4'te
kullanılacak, ve alternatifler bunu gerçekten kurtarmıyordu — `tiny_http` (~10
crate) async akışı ve Faz 4 paylaşımını kaybettiriyor, `tonic` ise daha da ağır.

Sayının çoğu ağ katmanından geliyor; `scan-core` hâlâ yalnızca rayon'a bağlı ve
çekirdek tarama yolu bu şişmeden etkilenmiyor. Karşılık olarak cron
ayrıştırıcısı, takvim aritmetiği ve sabit zamanlı token karşılaştırması elle
yazıldı — chrono, `cron` ve `subtle` eklenmedi.

**İzlenecek:** ikili boyutu ve musl derlemesi. Beklenti 5–8 MB; release
workflow'u ilk kez çalıştığında doğrulanmalı.

---

## K4 — Snapshot telde ham SQLite · 7 Eylül 2026

Ajan SQLite dosyasını olduğu gibi gönderir. Ara serileştirme formatı yok.
Transfer öncesi temiz, tek taramalık bir dosya kopyası üretilir; gövde zstd ile
sıkıştırılır.

**Uygulama notu (7 Eylül 2026).** İlk taslakta `VACUUM INTO` yazıyordu.
Uygulamada ATTACH + `INSERT ... SELECT` tercih edildi (`Store::export_snapshot`),
çünkü `VACUUM INTO` tüm veritabanını kopyalar; ATTACH ise yalnızca istenen
taramayı alır ve yeni dosya WAL'ı miras almaz — çağrı döndüğünde dosya kendi
kendine yeten tek parçadır. WAL tuzağı (aşağıda) böylece zaten çözülmüş oluyor.

**Neden.**

- Arena düzeni zaten telde taşınacak biçim. `entries` tablosu düz bir dizi; araya
  bir format koymak aynı satırları açıp yeniden paketlemek demek.
- Sürüm görüşmesi hazır: `PRAGMA user_version` daha yenisini açmayı reddediyor
  (`store/src/schema.rs`). Makineler arası tam olarak gereken kontrol bu.
- SQLite dosya formatı endian-bağımsız ve geriye dönük kararlı.
- **En büyük kazanç:** uzak snapshot'ı açan kod, yerel snapshot'ı açan kodun
  aynısı olur. `store::load` dosyanın nereden geldiğini bilmez.

**Ölçüm (7 Eylül 2026, `/usr/share`, 20 180 girdi).**

| | Ham | gzip -9 | zstd -19 |
|---|---|---|---|
| Toplam | 999 KB | 280 KB | 250 KB |
| Girdi başına | 49.5 B | 13.9 B | **12.4 B** |

1M dosyalık bir kök ≈ 50 MB ham, ~12 MB zstd. Gecelik transfer için kabul
edilebilir; "SQLite çok büyük" endişesi ölçümle kapandı.

**Kaçış yolu.** Boyut ileride sorun olursa `Accept-Encoding` üzerinden kompakt
bir format eklemek mevcut istemcileri kırmaz.

**Tuzak.** Şema WAL modunda (`schema.rs`). Canlı DB dosyasını doğrudan
göndermek eksik veri riski taşır — bu yüzden ham dosya asla olduğu gibi
gönderilmez, her zaman `export_snapshot` üzerinden geçer.

---

## K5 — Depo public · 7 Eylül 2026

Monorepo GitHub'da public, Apache-2.0.

**Neden.** Güven tarihle kurulur, lansman günü çevrilen bir bayrakla değil. Altı
aydır açık ve gerçek commit geçmişi olan bir ajan deposu, HN gönderisinden bir
hafta önce açılandan farklı okunur. Rekabet riski düşük: hendek tarama kodu değil
(rakiplerde zaten var), ajan + geçmiş + filo görünümü — kopyalamak için hedef
kullanıcılarını değiştirmeleri gerekir. Ayrıca GitHub Actions public depoda
ücretsiz.

**Sıra.** Önce K1 (İngilizce geçişi), sonra push. İlk izlenim Türkçe README
olmamalı.

---

## K6 — Kapasite "boş / toplam" olarak bildirilir · 7 Eylül 2026

Dosya sistemi kapasitesi kullanıcıya **boş yer ve toplam** olarak gösterilir;
"% dolu" olarak değil. `Capacity::unavailable()` hâlâ var ama ne olduğu
dokümante edildi ve arayüzde kullanılmıyor.

**Neden — ölçümle bulundu.** Faz 4 için kapasite eklendikten sonra CLI
"filesystem 70% full" yazıyordu. Aynı mount için `df` "6%" diyordu. İkisi de
aynı `total` ve `available` değerlerini görüyor; fark tanımda: APFS'te (ve btrfs
subvolume'larında, thin LVM havuzlarında) alan birimler arasında paylaşılır,
dolayısıyla `total - available` bu birimin değil konteynerin kullanımıdır. `df`
macOS'ta birimin kendi baytlarını raporlar.

Hangisi "doğru" sorusunun cevabı soruya bağlı: "yerim bitecek mi" için bizim
rakam, "buraya ne koydum" için `df`'in rakamı. Ama bir disk aracının tek
satmayan özelliği yanlış rakam olduğu için, `df` ile çelişen bir yüzdeyi
göstermek kabul edilemez. `available` her iki tarafta da aynı sayı, bu yüzden
arayüz onu gösteriyor.

**Sonuç.** WHY.md'nin doğruluk sözüne bir madde eklendi.

---

## K7 — Tahmin, dayanağı zayıfsa söylenmez · 7 Eylül 2026

Merkez servisin "N gün sonra dolar" tahmini yalnızca şu koşulların **hepsi**
sağlandığında gösterilir: ≥3 snapshot, ≥1 gün açıklık, doğrusal uyum r² ≥ 0.5,
ölçülmüş kapasite, ve cevabın 10 yıl içinde olması.

**Neden.** Disk kullanımı sık sık doğrusal değil. Bir log rotasyonu ya da tek
seferlik bir restore, eğimi hiçbir şey ifade etmeyen bir doğru üretir. Bir
saatlik üç örnek gelecek hafta hakkında hiçbir şey söylemez. Kendinden emin
yanlış bir tarih, hiç tarih olmamasından kötüdür — ve bir uyarı sistemi için
yanlış alarm, sistemin tamamının kapatılmasına yol açar.

Koşullar sağlanmazsa kolon boş kalır ve hedef sayfası nedenini yazar
("not extrapolated: 2 samples over 0.3 days, fit 0.12"). Eşikler
`spacetrace-hub/src/trend.rs` içinde `MIN_SAMPLES`, `MIN_SPAN_DAYS`, `MIN_R2`
olarak açıkça duruyor.

---

## K8 — Üç bileşenin indirilebilir dosyaları da public depoda · 8 Eylül 2026

Masaüstü ve hub kendi (private) depolarında derlenir, ama üretilen kurulum
dosyaları **bu deponun** release'lerine yayınlanır: `desktop-continuous` /
`desktop-v*` ve `hub-continuous` / `hub-v*` etiketleriyle.

**Neden.** Private bir deponun release varlıkları kimlik doğrulaması olmadan
indirilemiyor. K2 gereği masaüstü ve hub private, ama bir tanıtım sitesinin
indirme düğmesi ziyaretçiden token isteyemez. Kaynağın kapalı, dağıtımın açık
olması gerekiyordu; tek yol varlıkları public bir depoya taşımak.

Aynı yerde olmalarının ikinci bir faydası var: indirme sayfası tek bir GitHub
API çağrısıyla üç bileşenin de sürümünü öğreniyor.

**Bedeli.** Private depoların CI'ı public depoya yazabilmek için fine-grained
bir PAT (`RELEASE_TOKEN`) gerektiriyor — `GITHUB_TOKEN` depo dışına yazamıyor.
Sır yoksa iş akışı düşmüyor, varlıkları kendi deposunda yayınlayıp uyarı basıyor;
yani eksik sır sessiz bir başarısızlık değil.

**Sonuç.** Etiket ve varlık adları artık bir sözleşme: `website/download.html`
onlara doğrudan bağlanıyor ve `install.sh` dosya adını verilen sürümden kuruyor.
Yeniden adlandırmak indirme sayfasını kırar. Ayrıntı: [RELEASING.md](RELEASING.md).

---

## K9 — Her push bir "continuous" sürüm yayınlar · 8 Eylül 2026

`main`'e her push, sabit `continuous` etiketli bir ön-sürümü siler ve yeniden
oluşturur. Kararlı sürümler ayrıca `v*` etiketiyle kesiliyor.

**Neden.** "En son derleme" sabit bir URL'e sahip olmak zorunda: indirme sayfası
JavaScript olmadan da çalışan gerçek bağlantılar içeriyor ve `install.sh` dosya
adını sürümden kuruyor. Etiketi taşımak yerine silip yeniden oluşturmak, hem
etiketin `main`'i takip etmesini hem de matristen çıkarılan bir varlığın ölü
bağlantı olarak kalmamasını sağlıyor. Bedeli, yayın başına birkaç saniyelik bir
404 penceresi — sürekli kanal için kabul edilebilir.

`releases/latest` uç noktası ön-sürüm döndürmediği için `install.sh` kararlı
sürüm yokken `continuous`'a düşüyor ve bunu ekrana yazıyor. Aksi hâlde ilk
kararlı etikete kadar belgelenmiş tek satırlık kurulum komutu çalışmazdı.

---

## K10 — Yerelleştirme yalnızca GUI, site ve changelog · 9 Eylül 2026

K1 "i18n çerçevesi eklenmez, dizeler İngilizce ve gömülü kalır" diyordu ve bunu
kapsam dışı ilan edilmiş bir karar olarak niteliyordu. Bu karar **K1'i iptal
etmiyor, kapsamını daraltıyor**:

| Yüzey | Dil |
|-------|-----|
| Masaüstü uygulama arayüzü | `en tr it fr de` |
| Site | `en tr it fr de` (zaten öyleydi) |
| Changelog metinleri | `en tr it fr de` |
| CLI çıktısı, `--help`, hata mesajları | İngilizce |
| Ajan ve hub HTTP mesajları, `--help` | İngilizce |
| `install.sh`, systemd unit, TOML yorumları | İngilizce |
| Kod, kod yorumları, commit mesajları | değişmedi |

**Neden K1 tamamen açılmadı.** K1'in gerekçesinde çevrilecek yüzey olarak sayılan
her şey — `--help`, HTTP hata mesajları, TOML yorumları, systemd unit, kurulum
betiği — terminal ve sunucu yüzeyi. K1 bir GUI'yi hiç düşünmemişti, çünkü karar
verildiğinde ortada GUI yoktu. Terminal tarafındaki gerekçe hâlâ geçerli:
**çevrilmiş bir komut yanlış bilgidir**, ve bir kullanıcı `spacetrace diff`
çıktısını arama motoruna yapıştırdığında İngilizce olması işine yarar.

**Neden GUI tarafında geçerli değil.** Masaüstü ticari ürün ve hedef kitlesi HN
değil. Site zaten beş dil: kullanıcı ürünü Türkçe okuyup indiriyor, sonra
uygulamayı İngilizce açıyor. Bu tutarsızlığı K1 öngörmemişti.

**Sonuç.** Diller sitedekiyle aynı kümede tutuluyor — bir okuyucu sitede
İtalyanca seçip uygulamada İngilizceye düşmemeli. Changelog girdilerinde komut,
bayrak, dosya adı ve etiket **çevrilmiyor**; yalnızca etrafındaki cümle
çevriliyor.

---

## K11 — Changelog elle yazılan tek bir veri dosyası · 9 Eylül 2026

`crates/changelog/changelog.json` üç bileşenin de changelog'unun tek kaynağı.
Her deponun `CHANGELOG.md`'si, GitHub yayın notları, sitenin `/changelog`
sayfası ve masaüstündeki "Yenilikler" penceresi bundan üretiliyor.

**Neden commit'lerden üretilmiyor.** Commit "koda ne yaptım" der, Türkçe, bir
sonraki bakımcı için. Changelog "sana ne değişti" der. Farklı metinler, farklı
okuyucular. Üstelik teknik olarak da mümkün değil: bu depo bilinçli olarak
düzyazı commit kullanıyor (`APFS clone'larını bir kez say`), ve ölçüldü —
git-cliff, release-please ve cocogitto geçmişin **%100'ünü** "other" diye
sınıflar.

**Neden üç bileşen tek dosyada.** Masaüstü ve hub indirmelerini zaten çekirdek
deponun yayınlarına koyuyor, ve site tek bir public yer okuyor. Üç depoya bölmek
ya bir senkron işi ya da siteye bir token vermek demekti. Bedeli: masaüstünde
yapılan bir değişikliğin girdisi burada commit'lenmeli, ve masaüstü etiketlenmeden
önce çekirdek pin'i ilerletilmeli — yoksa ikilinin gömülü changelog'u kendi yayın
notlarından eski olur. Sıra [RELEASING.md](RELEASING.md)'de.

**Neden gömülü, indirilen değil.** Masaüstü "ne değişti"yi kendini
güncelledikten hemen sonra gösteriyor — yani ağının olmayabileceği tam o anda.
Boş bir "yenilikler" penceresi, hiç olmamasından kötü.

**`published: false`.** Etiketlenmemiş, indirilebilir dosyası olmayan geliştirme
kilometre taşı. İş yapıldığı için kaydediliyor, kimsenin kuramayacağı bir sürümü
varmış gibi göstermek yalan olacağı için işaretleniyor.
