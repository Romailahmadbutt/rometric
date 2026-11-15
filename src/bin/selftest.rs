//! Headless verification of the parts that can be tested without an audio
//! device: the embedded AutoEQ database and the DSP chain.
//!
//! Run with: cargo run --release --bin selftest

use rometric::autoeq::AutoEq;
use rometric::dsp::{EqChain, Params, N_BANDS};
use std::f32::consts::PI;

fn main() {
    let mut failures = 0;

    let db = match AutoEq::load() {
        Ok(db) => db,
        Err(e) => {
            eprintln!("FAIL  database: {e}");
            std::process::exit(1);
        }
    };
    println!("AutoEQ profiles loaded : {}", db.len());
    println!("grid points            : {}", db.freqs.len());

    check(&mut failures, "profile count is 6029", db.len() == 6029);

    let idx = db.index_of("1Custom SA02");
    check(&mut failures, "can locate a known profile", idx.is_some());

    if let Some(i) = idx {
        let at20 = db.curve_gain(i, 20.0);
        check(
            &mut failures,
            "curve matches source at 20 Hz (6.1 dB)",
            (at20 - 6.1).abs() < 1e-3,
        );
        let bands = db.bands_for(i);
        check(
            &mut failures,
            "11 band gains are finite",
            bands.iter().all(|g| g.is_finite()),
        );
        println!(
            "  sample bands for {}: {:?}",
            db.profiles[i].name,
            bands.map(|g| (g * 10.0).round() / 10.0)
        );
    }

    let flat = Params::default();
    let chain = EqChain::new(48_000.0, &flat);
    let flat_db = chain.response_db(1_000.0);
    check(&mut failures, "flat chain is 0 dB", flat_db.abs() < 0.01);

    // Profile categories must partition the whole library.
    let catalog = db.catalog();
    let brand_total: usize = catalog.brand_groups.iter().map(|g| g.len()).sum();
    let signature_total: usize = catalog.signature_groups.iter().map(|g| g.len()).sum();
    println!("\nprofile categories");
    println!("  brand folders        : {}", catalog.brand_groups.len());
    println!(
        "  sound folders        : {}",
        catalog.signature_groups.len()
    );
    for group in &catalog.signature_groups {
        println!("    {:<20} {:>5}", group.label, group.len());
    }
    println!(
        "  e.g. Sony            : {}",
        catalog
            .brand_groups
            .iter()
            .find(|g| g.label == "Sony")
            .map(|g| g.len())
            .unwrap_or(0)
    );
    check(
        &mut failures,
        "every profile has one brand folder",
        brand_total == db.len(),
    );
    check(
        &mut failures,
        "every profile has one sound folder",
        signature_total == db.len(),
    );
    check(
        &mut failures,
        "multi-word brands stay whole",
        catalog
            .brand_groups
            .iter()
            .any(|g| g.label == "Bowers & Wilkins"),
    );

    let mut boosted = Params::default();
    boosted.gains[5] = 6.0;
    boosted.limiter = false;
    let b = EqChain::new(48_000.0, &boosted);
    let peak_db = b.response_db(1_000.0);
    check(
        &mut failures,
        "1 kHz band reaches ~+6 dB",
        (peak_db - 6.0).abs() < 0.2,
    );

    // Ceiling check: +20 dB preamp into a limiter must stay below 0 dBFS.
    let loud = Params {
        preamp_db: 20.0,
        limiter: true,
        ..Default::default()
    };
    let mut chain = EqChain::new(48_000.0, &loud);
    let mut peak = 0.0f32;
    for n in 0..48_000 {
        let x = (2.0 * PI * 440.0 * n as f32 / 48_000.0).sin();
        let (l, r) = chain.process(x, x);
        peak = peak.max(l.abs()).max(r.abs());
        if !l.is_finite() || !r.is_finite() {
            check(&mut failures, "no NaN/Inf in output", false);
            break;
        }
    }
    check(&mut failures, "limiter caps output at 0 dBFS", peak <= 1.0);
    println!("  limiter peak: {:.4}", peak);

    // Interpolate a real profile into the 11 bands and run audio through it.
    if let Some(i) = idx {
        let p = Params {
            gains: db.bands_for(i),
            preamp_db: db.auto_preamp(i),
            limiter: true,
            ..Default::default()
        };
        let mut chain = EqChain::new(48_000.0, &p);
        let mut peak = 0.0f32;
        for n in 0..48_000 {
            let x = (2.0 * PI * 1_000.0 * n as f32 / 48_000.0).sin() * 0.5;
            let (l, r) = chain.process(x, x);
            peak = peak.max(l.abs()).max(r.abs());
        }
        check(
            &mut failures,
            "profile chain stays finite and bounded",
            peak <= 1.0,
        );
        println!(
            "  profile chain peak: {:.4} (preamp {:.1} dB)",
            peak, p.preamp_db
        );
        check(
            &mut failures,
            "preamp keeps headroom",
            p.preamp_db <= 0.0 && p.preamp_db >= -24.0,
        );
    }

    // Sanity: all gains at +12 dB must not blow up.
    let extreme = Params {
        gains: [12.0; N_BANDS],
        preamp_db: -12.0,
        ..Default::default()
    };
    let mut chain = EqChain::new(48_000.0, &extreme);
    let mut finite = true;
    for n in 0..96_000 {
        let x = (n as f32 * 0.01).sin();
        let (l, r) = chain.process(x, x * 0.5);
        finite &= l.is_finite() && r.is_finite();
    }
    check(&mut failures, "extreme boost is stable", finite);

    if failures == 0 {
        println!("\nAll checks passed.");
    } else {
        eprintln!("\n{failures} check(s) failed.");
        std::process::exit(1);
    }
}

fn check(failures: &mut u32, label: &str, ok: bool) {
    if ok {
        println!("PASS  {label}");
    } else {
        println!("FAIL  {label}");
        *failures += 1;
    }
}
