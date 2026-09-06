# spacetrace — Claude için proje notları

Disk kullanımını tarayan, SQLite anlık görüntüsüne yazan ve iki görüntüyü
karşılaştırarak **neyin büyüdüğünü** söyleyen bir araç. Rust workspace.

Bağlam okuması (kodda görünmeyen kararlar): [docs/WHY.md](docs/WHY.md) neden bu
ürün, [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) neden bu tasarım,
[docs/ROADMAP.md](docs/ROADMAP.md) fazlar, [TODO.md](TODO.md) sıradaki iş ve açık
kararlar. Bir özellik önerisini değerlendirirken WHY.md'deki **kapsam dışı**
listesine bak.

## Komutlar

```bash
cargo test --workspace                   # 32 test, hepsi geçmeli
cargo clippy --workspace --all-targets   # uyarısız olmalı
cargo fmt --all
cargo build --release                    # ikili: target/release/spacetrace
cargo check -p spacetrace-scan-core --target x86_64-pc-windows-msvc
```

Rust 1.85+ gerekir. Testler geçici dizinlerde **gerçek dosya sistemi** kullanır
(hardlink, symlink, izin hatası senaryoları dâhil), mock yok.

## Bozulmaması gereken değişmezler

Bunlar sessizce bozulabilir ve testler dışında fark edilmez:

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
4. **Hatalar yutulmaz.** Okunamayan yol sayılır ve örneklenir; tarama durmaz.
5. **Ajan hiçbir şeyi silmez.** Sunucuya kurulacak yazılımın güven kazanması için
   verilmiş bilinçli bir karar, eksik özellik değil.

## Kod ve depo alışkanlıkları

- **Kod yorumları İngilizce, kullanıcıya görünen dizeler Türkçe.** (Arayüz
  dilinin İngilizceye geçmesi açık bir karar — bkz. TODO.md.)
- Yorum *ne yaptığını* değil **neden öyle yaptığını** anlatır. Kodun kendisi ne
  yaptığını zaten söylüyor.
- Bağımlılık eklemekte cimri ol. Şu an tüm workspace 26 crate; bir bağımlılık
  eklemeden önce standart kütüphaneyle çözülüp çözülmediğine bak. Ajanın tek
  statik ikili olarak NAS'a kurulabilmesi gerekiyor.
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
| `cli` | `spacetrace` ikilisi |

Bağımlılık yönü tek yönlü. Ajan (Faz 2) bu üçünü kullanacak, `cli`'ye
bağlanmayacak.

## Sıradaki iş: Faz 2 (ajan)

Ürünün farklılaştığı yer masaüstü treemap değil, **uzak makine + zaman ekseni**.
Bu yüzden ajan görsel işlerden önce geliyor. Kırılım TODO.md'de.

Faz 2'ye başlamadan TODO.md'nin başındaki **açık kararlar** bölümüne bak:
arayüz dili, lisans modeli, ajan protokolü (HTTP mi gRPC mi), snapshot'ın telde
nasıl taşınacağı. Sonradan değiştirmesi pahalı olanlar bunlar.

## Bilinen eksikler

Bunlara denk gelirsen bug değil, bilinen borç (tam liste TODO.md'de):

- Windows'ta `alloc` mantıksal boyuta eşit ve hardlink dedupe kapalı
  (`TODO(win)`, `crates/scan-core/src/meta.rs`) — gerçek değerler için
  `GetFileInformationByHandleEx` ve `FileIdInfo` gerekiyor.
- APFS clone'ları tekilleştirilmiyor; btrfs/ZFS'te reflink ve sıkıştırma
  yüzünden ağaç yürüyüşü gerçek kullanımı yanlış raporluyor.
- Tarama tüm ağacı bellekte tutuyor; 10M+ dosyada bellek profili ölçülmedi.

## Bu depo dışındaki bağlam

Proje Cowork'te (claude.ai) başladı; oradaki oturum hafızası Claude Code'a
aktarılmaz, iki sistem ayrıdır. Aktarılması gereken her şey bu depodaki
belgelere yazıldı. Eylül 2026 pazar ve teknik araştırmasının özeti
[docs/RESEARCH.md](docs/RESEARCH.md) içinde.
