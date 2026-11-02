//! `printnest-cli`: headless PrintNest tool.
//!
//! Used by CI and by contributors to check catalogue data, project files and
//! photos without opening a window. No network. It only reads the file named
//! on the command line and only writes the file named by `--out` (refusing
//! to overwrite an existing file). Photos are decoded in a sandboxed copy of
//! this program (see `printnest-sandbox`); that is the only process it starts.

#![forbid(unsafe_code)]
// A CLI's job is to print.
#![allow(clippy::print_stdout, clippy::print_stderr)]

use printnest_model::catalogue::{Catalogue, Family, Product};
use printnest_model::project::Project;
use printnest_model::units::{Mm, Size, grade_photo};
use std::fs::OpenOptions;
use std::io::Write;
use std::process::ExitCode;

const USAGE: &str = "\
printnest-cli — PrintNest command-line tool

USAGE:
    printnest-cli catalogue [--family <family>]
    printnest-cli info <product> [--pages <n>] [--paper <id>] [--cover <id>]
    printnest-cli new <product> --out <file.json> [--title <text>]
    printnest-cli validate <file.json>
    printnest-cli dpi <product> <pixels-wide> <pixels-high>
    printnest-cli photo <image-file> [--product <product>]
    printnest-cli export <project-folder> --profile <profile> --out <folder>
    printnest-cli --version | --help

PROFILES (export):
    kiosk         JPEGs for photo kiosks and online labs
    shop          print-shop PDF with bleed and crop marks
    home-a4       home printer, A4 (large designs tile over several sheets)
    home-letter   home printer, US Letter

FAMILIES:
    print poster framed canvas book calendar card gift

EXIT CODES:
    0 ok · 1 invalid input or file · 2 usage error";

fn main() -> ExitCode {
    // If started as the sandboxed decoder, serve one request and exit.
    printnest_sandbox::maybe_run_worker();
    let args: Vec<String> = std::env::args().skip(1).collect();
    match run(&args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(Failure::Usage(msg)) => {
            eprintln!("error: {msg}\n\n{USAGE}");
            ExitCode::from(2)
        }
        Err(Failure::Invalid(msg)) => {
            eprintln!("error: {msg}");
            ExitCode::from(1)
        }
    }
}

enum Failure {
    Usage(String),
    Invalid(String),
}

fn usage(msg: impl Into<String>) -> Failure {
    Failure::Usage(msg.into())
}

fn bad(msg: impl std::fmt::Display) -> Failure {
    Failure::Invalid(msg.to_string())
}

/// Positional values and `--flag value` pairs.
type Parsed<'a> = (Vec<&'a str>, Vec<(&'a str, &'a str)>);

/// Splits `args` into positional values and `--flag value` pairs.
fn parse_flags<'a>(args: &'a [String], allowed: &[&str]) -> Result<Parsed<'a>, Failure> {
    let mut pos = Vec::new();
    let mut flags = Vec::new();
    let mut it = args.iter();
    while let Some(a) = it.next() {
        if let Some(name) = a.strip_prefix("--") {
            if !allowed.contains(&name) {
                return Err(usage(format!("unknown option --{name}")));
            }
            let value = it
                .next()
                .ok_or_else(|| usage(format!("--{name} needs a value")))?;
            flags.push((name, value.as_str()));
        } else {
            pos.push(a.as_str());
        }
    }
    Ok((pos, flags))
}

fn flag<'a>(flags: &[(&str, &'a str)], name: &str) -> Option<&'a str> {
    flags
        .iter()
        .rev()
        .find(|(n, _)| *n == name)
        .map(|(_, v)| *v)
}

fn run(args: &[String]) -> Result<(), Failure> {
    let catalogue = Catalogue::builtin().map_err(bad)?;
    let Some((cmd, rest)) = args.split_first() else {
        println!("{USAGE}");
        return Ok(());
    };
    match cmd.as_str() {
        "--help" | "-h" | "help" => {
            println!("{USAGE}");
            Ok(())
        }
        "--version" | "-V" => {
            println!("printnest-cli {}", env!("CARGO_PKG_VERSION"));
            Ok(())
        }
        "catalogue" => cmd_catalogue(&catalogue, rest),
        "info" => cmd_info(&catalogue, rest),
        "new" => cmd_new(&catalogue, rest),
        "validate" => cmd_validate(&catalogue, rest),
        "dpi" => cmd_dpi(&catalogue, rest),
        "photo" => cmd_photo(&catalogue, rest),
        "export" => cmd_export(&catalogue, rest),
        other => Err(usage(format!("unknown command {other:?}"))),
    }
}

fn parse_family(s: &str) -> Result<Family, Failure> {
    Family::ALL
        .into_iter()
        .find(|f| format!("{f:?}").eq_ignore_ascii_case(s))
        .ok_or_else(|| usage(format!("unknown family {s:?}")))
}

fn product<'a>(catalogue: &'a Catalogue, id: &str) -> Result<&'a Product, Failure> {
    catalogue.product(id).ok_or_else(|| {
        bad(format!(
            "unknown product {id:?} (see `printnest-cli catalogue`)"
        ))
    })
}

fn fmt_size(s: Size) -> String {
    format!("{:.1} × {:.1} mm", s.width.0, s.height.0)
}

fn cmd_catalogue(catalogue: &Catalogue, args: &[String]) -> Result<(), Failure> {
    let (pos, flags) = parse_flags(args, &["family"])?;
    if !pos.is_empty() {
        return Err(usage("catalogue takes no positional arguments"));
    }
    let only = flag(&flags, "family").map(parse_family).transpose()?;
    for family in Family::ALL {
        if only.is_some_and(|f| f != family) {
            continue;
        }
        println!("{}", family.label());
        for p in catalogue.in_family(family) {
            println!("  {:<26} {:<48} {}", p.id, p.name, fmt_size(p.trim()));
        }
        println!();
    }
    Ok(())
}

fn cmd_info(catalogue: &Catalogue, args: &[String]) -> Result<(), Failure> {
    let (pos, flags) = parse_flags(args, &["pages", "paper", "cover"])?;
    let [id] = pos.as_slice() else {
        return Err(usage("info needs exactly one product id"));
    };
    let p = product(catalogue, id)?;
    let pages = match flag(&flags, "pages") {
        Some(v) => v
            .parse::<u32>()
            .map_err(|_| usage(format!("--pages must be a whole number, got {v:?}")))?,
        None => p.default_pages(),
    };
    let paper_id = flag(&flags, "paper")
        .or_else(|| p.papers.first().map(String::as_str))
        .unwrap_or_default();
    if !p.allows_paper(paper_id) {
        return Err(bad(format!(
            "paper {paper_id:?} not available; choose from {}",
            p.papers.join(", ")
        )));
    }
    let paper = catalogue
        .paper(paper_id)
        .ok_or_else(|| bad(format!("unknown paper {paper_id:?}")))?;

    let trim = p.trim();
    let art = p.artwork();
    println!("{} ({})", p.name, p.id);
    println!("  family:       {}", p.family.label());
    println!(
        "  trim:         {} ({:.2} × {:.2} in)",
        fmt_size(trim),
        trim.width.inches(),
        trim.height.inches()
    );
    println!("  bleed:        {}", p.bleed());
    if let Some(w) = p.wrap_mm {
        println!("  canvas wrap:  {}", Mm(w));
    }
    println!("  artwork:      {}", fmt_size(art));
    println!(
        "  at 300 ppi:   {} × {} px",
        art.width.pixels_at(300.0),
        art.height.pixels_at(300.0)
    );
    println!("  safe margin:  {}", p.safe());
    if let Some(m) = p.mat_opening {
        println!("  mat opening:  {}", fmt_size(m.to_mm()));
    }
    if let Some(g) = p.gutter_mm {
        println!("  gutter:       {}", Mm(g));
    }
    if let Some(b) = p.binding_mm {
        println!("  binding:      {}", Mm(b));
    }
    let rule = p.page_rule();
    if rule.max > 1 {
        println!(
            "  pages:        {}–{} in steps of {} (using {pages})",
            rule.min, rule.max, rule.step
        );
        if !rule.allows(pages) {
            return Err(bad(format!(
                "{pages} pages is not allowed for this product"
            )));
        }
    }
    println!(
        "  paper:        {} ({} mm thick)",
        paper.name, paper.thickness_mm
    );
    if !p.covers.is_empty() {
        let covers: Vec<&str> = match flag(&flags, "cover") {
            Some(c) => vec![c],
            None => p.covers.iter().map(|c| c.id.as_str()).collect(),
        };
        for c in covers {
            let spine = p
                .spine_width(pages, paper, c)
                .ok_or_else(|| bad(format!("unknown cover {c:?}")))?;
            let spread = p
                .cover_spread(pages, paper, c)
                .ok_or_else(|| bad(format!("unknown cover {c:?}")))?;
            println!(
                "  cover {c:<8} spine {spine}, flat cover {}",
                fmt_size(spread)
            );
        }
    }
    let exports: Vec<&str> = p.exports.iter().map(|e| e.label()).collect();
    println!("  print via:    {}", exports.join("; "));
    println!(
        "  verified:     {}",
        if p.verified {
            "yes"
        } else {
            "not yet checked against a retailer"
        }
    );
    Ok(())
}

fn cmd_new(catalogue: &Catalogue, args: &[String]) -> Result<(), Failure> {
    let (pos, flags) = parse_flags(args, &["out", "title"])?;
    let [id] = pos.as_slice() else {
        return Err(usage("new needs exactly one product id"));
    };
    let out = flag(&flags, "out").ok_or_else(|| usage("new needs --out <file.json>"))?;
    let title = flag(&flags, "title").unwrap_or("Untitled");
    let project = Project::new_blank(catalogue, id, title).map_err(bad)?;
    let json = project.to_json().map_err(bad)?;
    // create_new: never overwrite an existing file.
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(out)
        .map_err(|e| bad(format!("could not create {out}: {e}")))?;
    file.write_all(json.as_bytes())
        .and_then(|()| file.write_all(b"\n"))
        .map_err(|e| bad(format!("could not write {out}: {e}")))?;
    println!("created {out} ({} pages)", project.pages.len());
    Ok(())
}

fn cmd_validate(catalogue: &Catalogue, args: &[String]) -> Result<(), Failure> {
    let (pos, _) = parse_flags(args, &[])?;
    let [path] = pos.as_slice() else {
        return Err(usage("validate needs exactly one file"));
    };
    let file = std::fs::File::open(path).map_err(|e| bad(format!("could not open {path}: {e}")))?;
    let project = Project::from_reader(file, catalogue).map_err(|e| bad(format!("{path}: {e}")))?;
    println!(
        "{path}: ok — {:?}, {} on {}, {} pages, {} photos",
        project.title,
        project.product,
        project.paper,
        project.pages.len(),
        project.photo_paths().len()
    );
    Ok(())
}

fn cmd_dpi(catalogue: &Catalogue, args: &[String]) -> Result<(), Failure> {
    let (pos, _) = parse_flags(args, &[])?;
    let [id, w, h] = pos.as_slice() else {
        return Err(usage("dpi needs <product> <pixels-wide> <pixels-high>"));
    };
    let p = product(catalogue, id)?;
    let parse = |v: &str| {
        v.parse::<u64>().ok().filter(|n| *n > 0).ok_or_else(|| {
            usage(format!(
                "pixel counts must be positive whole numbers, got {v:?}"
            ))
        })
    };
    let (w, h) = (parse(w)?, parse(h)?);
    let art = p.artwork();
    // Match orientation: a landscape photo on a portrait product gets rotated.
    let art = if (w >= h) == (art.width.0 >= art.height.0) {
        art
    } else {
        Size::new(art.height, art.width)
    };
    let (ppi, grade) = grade_photo(w, h, art).ok_or_else(|| bad("product has no area"))?;
    println!(
        "{w} × {h} px over {} ({}): {ppi:.0} ppi — {grade:?}",
        fmt_size(art),
        p.name
    );
    Ok(())
}

fn cmd_export(catalogue: &Catalogue, args: &[String]) -> Result<(), Failure> {
    use printnest_export::{Paper, Profile, Severity};
    let (pos, flags) = parse_flags(args, &["profile", "out"])?;
    let [path] = pos.as_slice() else {
        return Err(usage("export needs exactly one project folder"));
    };
    let profile = match flag(&flags, "profile") {
        Some("kiosk") => Profile::Kiosk,
        Some("shop") => Profile::PrintShop,
        Some("home-a4") => Profile::Home(Paper::A4),
        Some("home-letter") => Profile::Home(Paper::Letter),
        Some(other) => return Err(usage(format!("unknown profile {other:?}"))),
        None => return Err(usage("export needs --profile")),
    };
    let out = flag(&flags, "out").ok_or_else(|| usage("export needs --out <folder>"))?;
    let out = std::path::Path::new(out);
    if !out.is_dir() {
        return Err(bad(format!("{}: not a folder", out.display())));
    }
    let (folder, project) =
        printnest_document::open(std::path::Path::new(path), catalogue).map_err(bad)?;
    let product = product(catalogue, &project.product)?;
    let worker = printnest_sandbox::Worker::current_exe().map_err(bad)?;

    // Preflight needs each photo's full size: probe them in the sandbox.
    let mut sizes = std::collections::HashMap::new();
    for src in project.photo_paths() {
        if let Ok(bytes) = folder.read_photo(src)
            && let Ok(info) = worker.probe(&bytes)
        {
            sizes.insert(src.as_str().to_owned(), (info.width, info.height));
        }
    }
    let issues = printnest_export::preflight(&project, product, &sizes);
    for i in &issues {
        let tag = match i.severity {
            Severity::Problem => "PROBLEM",
            Severity::Warning => "warning",
        };
        eprintln!("{tag} (page {}): {}", i.page, i.message);
    }

    let mut loader = |src: &str, edge: u32| {
        let key = printnest_model::SafePath::new(src).ok()?;
        let bytes = folder.read_photo(&key).ok()?;
        let p = worker.preview(&bytes, edge).ok()?;
        Some(std::sync::Arc::new(printnest_render::Photo {
            width: p.width,
            height: p.height,
            rgba: p.rgba,
        }))
    };
    let mut progress = |done: usize, total: usize| {
        eprintln!("rendered page {done} of {total}");
        true
    };
    let report =
        printnest_export::export(&project, product, profile, out, &mut loader, &mut progress)
            .map_err(bad)?;
    println!(
        "exported to {} at {:.0} ppi:",
        report.folder.display(),
        report.ppi
    );
    for f in &report.files {
        println!(
            "  {}",
            f.file_name()
                .map(|n| n.to_string_lossy())
                .unwrap_or_default()
        );
    }
    if issues.iter().any(|i| i.severity == Severity::Problem) {
        return Err(bad("exported, but some photos are missing (see above)"));
    }
    Ok(())
}

fn cmd_photo(catalogue: &Catalogue, args: &[String]) -> Result<(), Failure> {
    let (pos, flags) = parse_flags(args, &["product"])?;
    let [path] = pos.as_slice() else {
        return Err(usage("photo needs exactly one image file"));
    };
    let bytes = printnest_sandbox::read_capped(std::path::Path::new(path))
        .map_err(|e| bad(format!("{path}: {e}")))?;
    let worker = printnest_sandbox::Worker::current_exe().map_err(bad)?;
    // A full decode (to a small preview) proves the whole file is readable,
    // not just its header. It happens inside the sandbox.
    let preview = worker
        .preview(&bytes, 256)
        .map_err(|e| bad(format!("{path}: {e}")))?;
    let info = preview.info;
    let mp = f64::from(info.width) * f64::from(info.height) / 1_000_000.0;
    println!(
        "{path}: {} {} × {} px ({mp:.1} MP), decoded {}",
        info.format.name(),
        info.width,
        info.height,
        printnest_sandbox::lockdown::expected().describe()
    );
    if let Some(id) = flag(&flags, "product") {
        let p = product(catalogue, id)?;
        let art = p.artwork();
        let art = if (info.width >= info.height) == (art.width.0 >= art.height.0) {
            art
        } else {
            Size::new(art.height, art.width)
        };
        let (ppi, grade) = grade_photo(u64::from(info.width), u64::from(info.height), art)
            .ok_or_else(|| bad("product has no area"))?;
        println!("  on {}: {ppi:.0} ppi full-bleed — {grade:?}", p.name);
    }
    Ok(())
}
