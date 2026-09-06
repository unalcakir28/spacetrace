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
cargo test --workspace                   # 107 test, hepsi geçmeli
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

- **Her şey İngilizce: kod yorumları, kullanıcıya görünen dizeler, `--help`
  metinleri, hata mesajları.** i18n katmanı yok ve planlanmıyor (bkz.
  [docs/DECISIONS.md](docs/DECISIONS.md) K1). Türkçe kalan tek yer: WHY,
  ROADMAP, TODO, RESEARCH, DECISIONS ve bu dosya — bunlar gerekçe belgeleri.
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
- `cargo clippy` uyarısı bırakma; CI `-D warnings` ile çalışıyor.

## Crate'ler

| Crate | Sorumluluk |
|-------|------------|
| `scan-core` | Tarama, ağaç modeli, platforma özel metadata. Hiçbir şeye bağlı değil. |
| `store` | SQLite anlık görüntü deposu, ncdu uyumlu dışa aktarım |
| `diff` | İki görüntüyü karşılaştırma, "suçlu klasör" tespiti |
| `cli` | `spacetrace` ikilisi (uzak kaynaklar dâhil) |
| `agent` | `spacetrace-agent` ikilisi: zamanlayıcı + HTTP servisi |

Bağımlılık yönü tek yönlü. Ajan (Faz 2) bu üçünü kullanır, `cli`'ye
bağlanmaz.

## Sıradaki iş: Faz 3 (masaüstü)

Faz 2 (ajan) çalışıyor: zamanlayıcı, HTTP servisi, push/pull, CLI `--remote`.
Kalan iki çıkış kriteri yalnızca gerçek makinelerde yapılabilir (bkz. TODO.md).

Fazlar arası kararlar kapandı ([docs/DECISIONS.md](docs/DECISIONS.md)); yeniden
açmadan önce oradaki gerekçeyi oku.

Ajanın kodunda dikkat edilecekler:

- Snapshot telde **ham SQLite**. `Store::export_snapshot` ATTACH ile tek taramayı
  ayrı dosyaya kopyalar; `import_snapshot` kimliği yeniden atar ama host/root/
  `started_at` üçlüsünü korur — yinelenme kontrolü bu üçlüye dayanıyor.
- Bir kök aynı anda yalnızca bir kez taranır (`Runner::try_claim`, HTTP'de 409).
- Zamanlayıcı UTC + sabit offset ile çalışır; saat dilimi veritabanı yok.

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
