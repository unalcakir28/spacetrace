# Sürüm ve dağıtım

Üç bileşenin (CLI + ajan, masaüstü, hub) nasıl derlenip nasıl indirilebilir hâle
geldiği. Bu belge gerekçe belgesi olduğu için Türkçe; üretilen her şey — sürüm
notları, indirme sayfası, kurulum talimatları — İngilizce (K1).

## Neden hepsi bu depoda yayınlanıyor

`spacetrace-desktop` ve `spacetrace-hub` private (K2: ticari kısım). **Private
bir deponun release varlıkları kimlik doğrulaması olmadan indirilemez.** İndirme
bağlantısı herkese açık olacaksa varlıklar public bir depoda durmak zorunda.

Çözüm: her depo kendi kodunu kendi CI'ında derler, çıktıyı **bu deponun
release'lerine** yayınlar. Kaynak private kalır, indirme public olur. Tek bir
yerde de olması iyi: indirme sayfası tek bir GitHub API çağrısıyla üç bileşenin
sürümünü öğreniyor.

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

### 1. `RELEASE_TOKEN` (iki private depo için)

Private bir depodaki `GITHUB_TOKEN` başka bir depoya yazamaz. Fine-grained bir
PAT gerekiyor:

1. <https://github.com/settings/personal-access-tokens/new>
2. Repository access → **Only select repositories** → `unalcakir28/spacetrace`
3. Permissions → Repository permissions → **Contents: Read and write**
4. Süreyi seçip oluştur, tokenı kopyala

Sonra iki depoya da sır olarak ekle:

```bash
gh secret set RELEASE_TOKEN --repo unalcakir28/spacetrace-desktop
gh secret set RELEASE_TOKEN --repo unalcakir28/spacetrace-hub
```

Sır yoksa iş akışı **hata vermiyor**: varlıkları kendi private deposunda
yayınlıyor ve bir uyarı basıyor. Yani ilk push'lar boşa gitmez, sadece indirme
bağlantıları public olmaz.

Token süresi dolduğunda yayınlama adımı 403 ile düşer. Yenile ve aynı komutu
tekrar çalıştır.

### 2. GHCR paket görünürlüğü

Private bir depodan oluşturulan GHCR paketi private başlıyor. `docker pull`
kimlik doğrulaması istemesin diye bir kez:

Paket sayfası → **Package settings** → Change visibility → **Public**.

- <https://github.com/users/unalcakir28/packages/container/spacetrace/settings>
- <https://github.com/users/unalcakir28/packages/container/spacetrace-hub/settings>

`spacetrace` imajı bu public depodan geldiği için zaten public olabilir; yine de
ilk yayından sonra kontrol et.

### 3. GitHub Pages — **yapıldı**

`GITHUB_TOKEN`'ın hiç var olmamış bir Pages sitesini oluşturma izni yok
("Resource not accessible by integration"), yani `configure-pages` bunu
kendisi başlatamıyor. Bir kez açıldı:

```bash
gh api -X POST repos/unalcakir28/spacetrace/pages -f build_type=workflow
```

Site <https://unalcakir28.github.io/spacetrace/> adresinde. Bir daha
gerekmiyor; iş akışı bundan sonra yalnızca yapılandırmayı okuyor.

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

`website/` bir **Astro** projesi. Beş dil (en, tr, it, fr, de) ve 31 statik
sayfa üretiyor; `pages.yml` bunu derleyip Pages'e yüklüyor.

Kolay bozulan yerler:

- **Sözlükler İngilizceye karşı tipli.** `src/i18n/ui/en.ts` kaynak; diğer dört
  dil `Dictionary` tipiyle ona uyuyor. Bir dile eklenip diğerlerinde unutulan
  anahtar `yarn typecheck` ile derleme hatası veriyor, canlı sayfada boşluk
  olarak değil. İş akışı bu yüzden `build`'den önce `typecheck` çalıştırıyor.
- **`yarn check` yazma.** yarn 1.x'in kendi yerleşik komutu ve script'i
  gölgeliyor — sessizce "Folder in sync" der ve tip denetimi hiç çalışmaz.
  Script'in adı bu yüzden `typecheck`.
- **İndirme sözleşmesi tek yerde:** `src/data/releases.ts`. Etiket adları ve
  varlık adları yukarıdaki tablodakilerle aynı olmak zorunda; oradaki bir
  yeniden adlandırma her indirme bağlantısını kırar.
- **JS kapalıyken de çalışan bir indirme sayfası bırakmak şart.** Bağlantılar
  işaretlemede gerçek dosyalara işaret ediyor (`continuous` etiketleri hiç
  kımıldamıyor); `src/scripts/releases.ts` yalnızca üzerine bilgi ekliyor —
  sürüm, tarih, boyut, ve kararlı sürüm çıktığında bağlantıların ona
  yükseltilmesi. Her adım korumalı, hata sessizce yutuluyor.
- **Etkileşimli treemap tek React adası** (`src/components/demo/`). Sunucuda da
  makul bir geometriyle çiziliyor, yani JS olmadan da dolu görünüyor.
- **`base: /spacetrace`** — her iç bağlantı `localeUrl()` üzerinden geçiyor. Elle
  yazılan bir yol `astro dev`'de çalışır, üretimde 404 verir.
