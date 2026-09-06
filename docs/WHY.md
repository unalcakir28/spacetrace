# Neden spacetrace?

Bu belge projenin çıkış noktasını, hangi boşluğu doldurduğunu ve neyi
**yapmayacağını** anlatır. Kod yazmadan önce verilen kararların gerekçesi burada;
bir özellik önerisi geldiğinde "bu bizim işimiz mi?" sorusu buradan cevaplanır.

## Çıkış noktası

Fikir başlangıçta "her platformda çalışan bir TreeSize klonu" idi. Eylül 2026'da
yapılan pazar ve teknik araştırma iki şeyi net biçimde gösterdi:

**1. Masaüstü treemap pazarı artık boş değil.** 2025–2026'da tam olarak bu
tanımla üç ürün çıktı:

| Ürün | Platform | Fiyat | Not |
|------|----------|-------|-----|
| FreeSize | Win/mac/Linux | Ücretsiz, Pro CHF 29/yıl | Kendini "TreeSize alternatifi" diye pazarlıyor |
| Diskaroo | Win/mac/Linux | $19.99 tek seferlik | Ayrı native kod tabanları (Swift + WPF) |
| DiskRaptor | Win/mac/Linux | Ücretsiz, MIT | Rust + Tauri |

Bunların üstüne dördüncü bir masaüstü treemap koymak, "daha iyi yaptık"
iddiasından ibaret bir ürün demek. Rakiplerden biri ücretsizken bu iddia
savunulamaz: eksik gördüğün özelliği ekler, fiyatı sıfır tutar.

**2. Asıl boşluk uzak makinelerde ve zaman ekseninde.** Pazardaki her araç tek
bir soruyu cevaplıyor: *"şu an önümdeki diskte ne var?"* Kimse şunu cevaplamıyor:
*"geçen haftadan beri ne değişti, ve hangi makinede?"*

| Yetenek | FreeSize | Diskaroo | WizTree | TreeSize Pro | SpaceObServer | ncdu/gdu |
|---|---|---|---|---|---|---|
| Yerel treemap | ✅ | ✅ | yalnız Win | yalnız Win | yalnız Win | TUI |
| Uzak sunucu/NAS, GUI'de | — | — | — | SSH/UNC, Win'den | ✅ | SSH ile elle |
| Konteyner / Docker volume | — | — | — | — | — | elle |
| Tarama geçmişi, diff | — | — | — | Pro'da kısmen | ✅ | — |
| Çoklu makine tek görünüm | — | — | — | — | ✅ | — |
| Fiyat | 0 / CHF 29 | $19.99 | 0 / $25+ | $49/yıl | **$600+/yıl** | 0 |

Tabloda tek bir sütun tüm satırları dolduruyor: SpaceObServer, yılda $600'dan
başlayan kurumsal bir ürün. Onun altında, ncdu'nun üstünde koca bir boşluk var.
Orada duran kişi bugün SSH'a girip `ncdu` çalıştırıyor, çıktıyı saklayamıyor,
iki hafta öncesiyle karşılaştıramıyor.

## Hedef kullanıcı

Sırayla, öncelik sırasına göre:

1. **Homelab / VPS sahibi geliştirici.** Birkaç Hetzner sunucusu, bir Proxmox
   host'u, bir NAS, onlarca Docker volume'u. Disk dolduğunda `du -sh *` ile
   ağaçta gezinerek suçluyu arıyor. Aynı şeyi bir hafta sonra tekrar yapıyor.
2. **5–50 sunucu yöneten küçük IT ekibi.** SpaceObServer'ı pahalı buluyor,
   Zabbix disk uyarısı "%85 doldu" diyor ama *neyin* büyüdüğünü söylemiyor.
3. **Masaüstü kullanıcısı.** Yerel diskini temizlemek isteyen herkes. Bu grup
   ürünün giriş kapısı ve topluluk kaynağı; para buradan gelmiyor.

## Ürün tanımı

> Bilgisayarındaki, sunucularındaki, NAS'ındaki ve konteynerlerindeki disk
> kullanımını tek yerde gör, zaman içinde takip et, neyin büyüdüğünü öğren.

Yerel analiz ücretsiz ve iyi olacak — o giriş kapısı. Ajan, geçmiş ve çoklu
makine görünümü ürünün savunulabilir kısmı: FreeSize'ın bunu kopyalaması için
sunucu tarafı yazması, bir depo formatı tasarlaması ve hedef kullanıcısını
değiştirmesi gerekir.

## Neden mobil yok

İlk fikirde iOS ve Android da vardı. Teknik araştırma bunun mümkün olmadığını
gösterdi:

- **iOS:** Uygulama yalnızca kendi sandbox'ını görür. Başka yerlere erişim
  kullanıcının Files'tan seçtiği klasörün alt ağacıyla sınırlıdır
  (security-scoped bookmark). Ayarlar → iPhone Depolama'daki uygulama başına
  hesap için public API yoktur. App Store'daki hiçbir uygulama sistem genelinde
  klasör bazlı kullanım gösteremez.
- **Android:** Tam ağaç taraması `MANAGE_EXTERNAL_STORAGE` ister. Google Play bu
  izni yalnızca listeli kategorilere verir (dosya yöneticisi, yedekleme,
  antivirüs, belge yönetimi, cihaz içi arama, disk şifreleme, cihaz taşıma) ve
  **disk analizörü bu listede yoktur**. İzin alınsa bile `Android/data` ve
  `Android/obb` kapalıdır. DiskUsage uygulaması tam bu yüzden Play'den kalktı.

Mobilde disk analizi yapılamıyorsa mobil uygulamanın gerekçesi de yok. İleride
ajan verisini telefondan görmek istenirse, merkez servisin sunacağı bir web
görünümü yeterli olur; native mobil kod yazmaya gerek kalmaz.

## Kapsam dışı (non-goals)

Bunlar bilinçli olarak yapılmayacak. Bir özellik isteği bu listeye giriyorsa
cevabı hayır:

- **Temizleyici değil.** "Junk cleaner", "önbellek temizle", "bir tıkla hızlan"
  yok. Ajan ilk sürümde hiçbir şeyi silmez, yalnızca okur ve rapor eder — bir
  sunucuya kurulacak yazılımın güven kazanması için en kolay yol bu.
- **Dosya yöneticisi değil.** Kopyalama, taşıma, önizleme, arşivleme yok.
- **Antivirüs / güvenlik tarayıcısı değil.**
- **Yedekleme aracı değil.** Snapshot burada "disk kullanımının fotoğrafı"
  demek, verinin kopyası değil.
- **Mobil uygulama yok** (yukarıdaki gerekçe).
- **Zorunlu bulut yok.** Merkez servis isteğe bağlı ve self-host edilebilir
  olacak; hiçbir veri dışarı çıkmadan tam işlevsel kullanılabilmeli.

## Konumlandırma ve fiyat hipotezi

Araştırmadaki fiyat çıpaları: DaisyDisk $9.99 tek seferlik, Diskaroo $19.99,
TreeSize Personal $50, TreeSize Pro $49/yıl, SpaceObServer $600+/yıl.

| Kademe | Kapsam | Fiyat fikri |
|--------|--------|-------------|
| Free | Masaüstü yerel analiz, CLI, SSH ile tek makine, yerel geçmiş | $0 |
| Pro | Sınırsız ajan, zamanlanmış snapshot, diff/trend, duplicate | $29–39 kalıcı veya $19/yıl |
| Team | Merkez servis, fleet panosu, uyarılar | $5–8/sunucu/ay |

Bunlar doğrulanmamış hipotez; ilk 100 kullanıcıyla test edilecek. Bir not:
JAM Software 2025'te TreeSize'ı aboneliğe çevirdi ve Temmuz 2026'da kalıcı lisans
sahiplerine güncelleme vermeyi kesti. Forumlardaki tepki, **kalıcı lisans
seçeneği sunmanın** kendi başına bir edinim kanalı olduğunu gösteriyor.

## Nasıl kazanırız / nasıl kaybederiz

**Kazanma koşulları**
- Bir geliştirici tek komutla sunucusuna ajan kurabiliyor ve bir hafta sonra
  "şu klasör 40 GB büyüdü" cevabını alıyor.
- Yerel tarama WizTree/FreeSize kadar hızlı ve doğru; kimse "yavaş" demiyor.
- r/selfhosted ve HN'de "ncdu'yu zamanla karşılaştıran şey" diye anılıyoruz.

**Kaybetme senaryoları**
- Windows'ta MFT okumadan çıkarız, WizTree ile yan yana konur ve yavaş kalırız.
- Kapsam şişer: ajan + merkez + masaüstü aynı anda yapılmaya çalışılır, hiçbiri
  bitmez. (Panzehir: merkez MVP'de yok, masaüstü doğrudan ajana bağlanır.)
- Ajan güven kazanamaz. (Panzehir: çekirdek ve ajan açık kaynak, silme yetkisi
  yok, telemetri yok.)
- Konumlandırma kayar ve dördüncü bir masaüstü treemap'ine döneriz.

## Doğruluk sözü

Bir disk aracının tek satmayan özelliği yanlış rakamdır. Bu yüzden:

- `size` (mantıksal) ve `alloc` (diskte) ayrı ayrı raporlanır, karıştırılmaz.
- Sabit bağlantılar bir kez sayılır, sembolik bağlantılar izlenmez.
- Toplamlar `du` ile birebir eşleşmelidir; bu bir test koşuludur, temenni değil.
- Okunamayan yollar sessizce atlanmaz, sayılır ve raporlanır.

## Kaynaklar

Yukarıdaki fiyat, tarih ve politika bilgileri 6 Eylül 2026'da yapılan
araştırmadan. Ayrıntılı rakip tabloları, framework karşılaştırması ve tarama
teknikleri için o araştırma raporuna bakın. Rakamlar hızla değişir; karar
öncesi yeniden doğrulayın.
