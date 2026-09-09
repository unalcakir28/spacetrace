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

Üç depoda da aynı: sürümü yükselt, etiketle, push et.

```bash
# CLI + ajan (bu depo): Cargo.toml içindeki workspace.package.version
git tag v0.2.0 && git push origin v0.2.0

# masaüstü: package.json, src-tauri/Cargo.toml ve src-tauri/tauri.conf.json
git tag v0.2.0 && git push origin v0.2.0

# hub: Cargo.toml
git tag v0.2.0 && git push origin v0.2.0
```

Etiket `v*` olduğu sürece iş akışı kararlı kanala yayınlıyor. Masaüstü ve hub
etiketleri bu depoda `desktop-v0.2.0` / `hub-v0.2.0` olarak görünür — aynı isim
alanında üç bileşen olduğu için.

Kuru çalışma: `workflow_dispatch` ile bir etiket adı ver. Derleme yapılır,
yayınlama adımı atlanır — `publish` kutusunu işaretlemezsen.

`publish: true` ile elle yayınlama, `paths-ignore` filtresinin bir etiket
push'unu atladığı durumun çıkış yolu (aşağıya bak). Etiket hedef depoda
default branch'in ucunda oluşturulur.

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
