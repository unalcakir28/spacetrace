use std::path::PathBuf;

use clap::{Args, Parser, Subcommand};

#[derive(Parser, Debug)]
#[command(
    name = "spacetrace",
    version,
    about = "Disk kullanımını tara, anlık görüntüle ve neyin büyüdüğünü gör",
    long_about = "spacetrace bir diski veya klasörü tarar, sonucu bir SQLite anlık \
görüntüsüne yazar ve iki anlık görüntüyü karşılaştırarak alanı neyin yediğini gösterir. \
Aynı ikili sunucuda, NAS'ta ve konteynerde de çalışır."
)]
pub struct Cli {
    /// Anlık görüntü veritabanı (varsayılan: kullanıcı veri klasörü)
    #[arg(long, global = true, value_name = "DOSYA")]
    pub db: Option<PathBuf>,

    /// Çıktıyı JSON olarak ver
    #[arg(long, global = true)]
    pub json: bool,

    #[command(subcommand)]
    pub command: Command,
}

#[derive(Subcommand, Debug)]
pub enum Command {
    /// Bir yolu tara ve özeti yazdır
    Scan(ScanArgs),
    /// Bir anlık görüntüde veya taze taramada klasörleri boyuta göre listele
    Ls(LsArgs),
    /// Kayıtlı anlık görüntüleri listele
    Scans,
    /// İki anlık görüntüyü karşılaştır: ne büyüdü, ne küçüldü
    Diff(DiffArgs),
    /// Bir anlık görüntüyü ncdu uyumlu JSON olarak dışa aktar
    Export(ExportArgs),
    /// Hedef başına en yeni N anlık görüntü dışındakileri sil
    Prune(PruneArgs),
    /// Bir anlık görüntüyü sil
    Rm(RmArgs),
}

#[derive(Args, Debug)]
pub struct ScanArgs {
    /// Taranacak yol
    #[arg(default_value = ".")]
    pub path: PathBuf,

    /// Sonucu veritabanına kaydet
    #[arg(long)]
    pub save: bool,

    /// Kayıt için etiket (ör. "haftalık")
    #[arg(long, value_name = "METİN")]
    pub label: Option<String>,

    #[command(flatten)]
    pub walk: WalkArgs,

    /// Kaç büyük girdi gösterilsin
    #[arg(long, default_value_t = 15, value_name = "N")]
    pub top: usize,

    /// Sonucu ayrıca ncdu uyumlu JSON olarak bu dosyaya yaz ("-" = stdout)
    #[arg(long, value_name = "DOSYA")]
    pub ncdu: Option<PathBuf>,
}

#[derive(Args, Debug)]
pub struct WalkArgs {
    /// Bu adlardaki klasörlere hiç girme (tekrarlanabilir)
    #[arg(long = "exclude", value_name = "AD")]
    pub exclude: Vec<String>,

    /// Dosya sistemi sınırlarını aşma (du -x gibi)
    #[arg(short = 'x', long = "one-file-system")]
    pub one_file_system: bool,

    /// Bu derinlikten aşağı inme
    #[arg(long, value_name = "N")]
    pub depth: Option<usize>,

    /// Sabit bağlantıları (hardlink) tekilleştirme, her kopyayı say
    #[arg(long)]
    pub no_dedupe: bool,
}

#[derive(Args, Debug)]
pub struct LsArgs {
    /// Taranacak yol (--scan verilmediyse)
    #[arg(default_value = ".")]
    pub path: PathBuf,

    /// Taze tarama yerine kayıtlı anlık görüntüyü kullan
    #[arg(long, value_name = "ID")]
    pub scan: Option<i64>,

    /// Anlık görüntü içinde gösterilecek alt yol
    #[arg(long, value_name = "YOL")]
    pub subpath: Option<String>,

    /// Kaç satır gösterilsin
    #[arg(long, default_value_t = 20, value_name = "N")]
    pub top: usize,

    #[command(flatten)]
    pub walk: WalkArgs,
}

#[derive(Args, Debug)]
pub struct DiffArgs {
    /// Eski anlık görüntü kimliği
    #[arg(long, value_name = "ID")]
    pub from: Option<i64>,

    /// Yeni anlık görüntü kimliği (verilmezse şimdi taranır)
    #[arg(long, value_name = "ID")]
    pub to: Option<i64>,

    /// Bu yolun son iki anlık görüntüsünü karşılaştır
    #[arg(long, value_name = "YOL")]
    pub path: Option<PathBuf>,

    /// Bu yolun son kaydı ile diskin şu anki hâlini karşılaştır
    #[arg(long, conflicts_with_all = ["from", "to"], value_name = "YOL")]
    pub since_last: Option<PathBuf>,

    /// Bundan küçük değişiklikleri yok say (ör. 10M, 500K)
    #[arg(long, default_value = "1M", value_name = "BOYUT")]
    pub min: String,

    /// Dosyaları da raporla, yalnızca klasörleri değil
    #[arg(long)]
    pub files: bool,

    /// Raporda bu derinlikten aşağı inme (suçlu klasörü kaç seviyeye kadar ara)
    #[arg(long, value_name = "N")]
    pub depth: Option<usize>,

    /// Kaç satır gösterilsin
    #[arg(long, default_value_t = 25, value_name = "N")]
    pub top: usize,
}

#[derive(Args, Debug)]
pub struct ExportArgs {
    /// Dışa aktarılacak anlık görüntü
    #[arg(long, value_name = "ID")]
    pub scan: i64,

    /// Hedef dosya ("-" = stdout)
    #[arg(long, default_value = "-", value_name = "DOSYA")]
    pub out: PathBuf,
}

#[derive(Args, Debug)]
pub struct PruneArgs {
    /// Hedef başına saklanacak anlık görüntü sayısı
    #[arg(long, default_value_t = 10, value_name = "N")]
    pub keep: usize,
}

#[derive(Args, Debug)]
pub struct RmArgs {
    /// Silinecek anlık görüntü kimliği
    pub id: i64,
}

/// `10M`, `500K`, `2G` ya da düz bayt sayısı.
pub fn parse_size(s: &str) -> anyhow::Result<u64> {
    let s = s.trim();
    let (num, mult) = match s.chars().last() {
        Some('K') | Some('k') => (&s[..s.len() - 1], 1024),
        Some('M') | Some('m') => (&s[..s.len() - 1], 1024 * 1024),
        Some('G') | Some('g') => (&s[..s.len() - 1], 1024 * 1024 * 1024),
        Some('T') | Some('t') => (&s[..s.len() - 1], 1024_u64.pow(4)),
        Some('B') | Some('b') => (&s[..s.len() - 1], 1),
        _ => (s, 1),
    };
    let value: f64 = num
        .trim()
        .parse()
        .map_err(|_| anyhow::anyhow!("boyut anlaşılamadı: {s:?} (ör. 10M, 500K, 2G)"))?;
    anyhow::ensure!(value >= 0.0, "boyut negatif olamaz: {s:?}");
    Ok((value * mult as f64) as u64)
}

impl WalkArgs {
    pub fn to_options(&self) -> spacetrace_scan_core::ScanOptions {
        spacetrace_scan_core::ScanOptions {
            exclude_names: self.exclude.clone(),
            one_filesystem: self.one_file_system,
            max_depth: self.depth,
            dedupe_hardlinks: !self.no_dedupe,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes_accept_units() {
        assert_eq!(parse_size("1024").unwrap(), 1024);
        assert_eq!(parse_size("1K").unwrap(), 1024);
        assert_eq!(parse_size("10M").unwrap(), 10 * 1024 * 1024);
        assert_eq!(parse_size("1.5G").unwrap(), 1610612736);
        assert_eq!(parse_size(" 2g ").unwrap(), 2 * 1024 * 1024 * 1024);
    }

    #[test]
    fn bad_sizes_are_rejected() {
        assert!(parse_size("abc").is_err());
        assert!(parse_size("-5M").is_err());
    }

    #[test]
    fn the_cli_definition_is_valid() {
        use clap::CommandFactory;
        Cli::command().debug_assert();
    }
}
