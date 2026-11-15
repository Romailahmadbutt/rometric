use rometric::catalog::{
    brand_of, filter_groups, signature_of, Grouping, ProfileCatalog, Signature,
};

fn log_grid() -> Vec<f32> {
    (0..=120)
        .map(|index| 20.0 * 1000f32.powf(index as f32 / 120.0))
        .collect()
}

fn curve(freqs: &[f32], f: impl Fn(f32) -> f32) -> Vec<f32> {
    freqs.iter().map(|freq| f(*freq)).collect()
}

#[test]
fn brand_names_keep_multiword_brands_whole() {
    assert_eq!(brand_of("Sony WH-1000XM4"), "Sony");
    assert_eq!(brand_of("Sennheiser HD 600"), "Sennheiser");
    assert_eq!(brand_of("64 Audio A12t"), "64 Audio");
    assert_eq!(brand_of("Bowers & Wilkins P7"), "Bowers & Wilkins");
    assert_eq!(brand_of("Astell & Kern Rosie (max)"), "Astell & Kern");
    assert_eq!(brand_of("Queen of Audio Mojito"), "Queen of Audio");
    assert_eq!(brand_of("Audio-Technica ATH-M50x"), "Audio-Technica");
    assert_eq!(brand_of("Moondrop x Crinacle DUSK"), "Moondrop");
    assert_eq!(brand_of("  Sony  "), "Sony");
    assert_eq!(brand_of(""), "Unknown");
}

#[test]
fn signature_follows_the_shape_of_the_correction() {
    let freqs = log_grid();

    // Correction that lifts bass only.
    let bass_lift = curve(&freqs, |f| if f < 200.0 { 6.0 } else { 0.0 });
    assert_eq!(signature_of(&freqs, &bass_lift), Signature::AddsBass);

    // Correction that removes bass only (headphone is bass-heavy).
    let bass_cut = curve(&freqs, |f| if f < 200.0 { -5.0 } else { 0.0 });
    assert_eq!(signature_of(&freqs, &bass_cut), Signature::CutsBass);

    let treble_lift = curve(&freqs, |f| if f > 4_000.0 { 5.0 } else { 0.0 });
    assert_eq!(signature_of(&freqs, &treble_lift), Signature::AddsTreble);

    let mids = curve(&freqs, |f| {
        if (300.0..=2_000.0).contains(&f) {
            -4.0
        } else {
            0.0
        }
    });
    assert_eq!(signature_of(&freqs, &mids), Signature::CutsMids);

    let flat = vec![0.0; freqs.len()];
    assert_eq!(signature_of(&freqs, &flat), Signature::Neutral);
}

#[test]
fn signature_picks_the_dominant_region_when_several_move() {
    let freqs = log_grid();
    let tilted = curve(&freqs, |f| {
        let bass = if f < 200.0 { 2.0 } else { 0.0 };
        let treble = if f > 4_000.0 { 6.0 } else { 0.0 };
        bass + treble
    });
    assert_eq!(signature_of(&freqs, &tilted), Signature::AddsTreble);
}

#[test]
fn catalog_places_every_profile_in_exactly_one_group() {
    let freqs = log_grid();
    let names: Vec<String> = [
        "Sony WH-1000XM4",
        "Sony WF-1000XM5",
        "64 Audio A12t",
        "Sennheiser HD 600",
        "Unique Earbud One",
    ]
    .iter()
    .map(|name| name.to_string())
    .collect();
    let curves: Vec<Vec<f32>> = vec![
        curve(&freqs, |f| if f < 200.0 { 5.0 } else { 0.0 }),
        curve(&freqs, |f| if f < 200.0 { 1.0 } else { 0.0 }),
        curve(&freqs, |f| if f > 4_000.0 { 4.0 } else { 0.0 }),
        vec![0.0; freqs.len()],
        curve(&freqs, |f| if f < 200.0 { -6.0 } else { 0.0 }),
    ];

    let catalog = ProfileCatalog::build(&names, &freqs, &curves);

    let mut brand_hits = vec![0usize; names.len()];
    for group in &catalog.brand_groups {
        for index in &group.profiles {
            brand_hits[*index] += 1;
        }
    }
    assert_eq!(brand_hits, vec![1; names.len()]);

    let mut signature_hits = vec![0usize; names.len()];
    for group in &catalog.signature_groups {
        for index in &group.profiles {
            signature_hits[*index] += 1;
        }
    }
    assert_eq!(signature_hits, vec![1; names.len()]);

    assert_eq!(catalog.signatures[0], Signature::AddsBass);
    assert_eq!(catalog.signatures[2], Signature::AddsTreble);
    assert_eq!(catalog.brands[1], "Sony");
}

#[test]
fn brand_groups_are_alphabetical_and_singletons_are_collated_last() {
    let freqs = log_grid();
    let names: Vec<String> = ["Sony One", "Sony Two", "AKG One", "1Custom SA02"]
        .iter()
        .map(|name| name.to_string())
        .collect();
    let curves = vec![vec![0.0; freqs.len()]; names.len()];
    let catalog = ProfileCatalog::build(&names, &freqs, &curves);

    let labels: Vec<&str> = catalog
        .brand_groups
        .iter()
        .map(|g| g.label.as_str())
        .collect();
    assert_eq!(labels, vec!["Sony", "Other brands"]);
    assert_eq!(catalog.brand_groups[0].profiles.len(), 2);
    assert!(catalog.brand_groups[1].is_other);
}

#[test]
fn signature_groups_use_a_stable_order_and_omit_empty_buckets() {
    let freqs = log_grid();
    let names: Vec<String> = ["Bass Head", "Flat Can"]
        .iter()
        .map(|n| n.to_string())
        .collect();
    let curves = vec![
        curve(&freqs, |f| if f < 200.0 { 5.0 } else { 0.0 }),
        vec![0.0; freqs.len()],
    ];
    let catalog = ProfileCatalog::build(&names, &freqs, &curves);

    let labels: Vec<&str> = catalog
        .signature_groups
        .iter()
        .map(|g| g.label.as_str())
        .collect();
    assert_eq!(labels, vec!["Adds bass", "Neutral correction"]);
}

#[test]
fn search_filters_each_group_and_drops_empty_ones() {
    let freqs = log_grid();
    let names: Vec<String> = [
        "Sony WH-1000XM4",
        "Sony WF-1000XM5",
        "Sennheiser HD 600",
        "AKG K371",
    ]
    .iter()
    .map(|n| n.to_string())
    .collect();
    let curves = vec![vec![0.0; freqs.len()]; names.len()];
    let catalog = ProfileCatalog::build(&names, &freqs, &curves);

    let filtered = filter_groups(&catalog.brand_groups, &names, "wf");
    assert_eq!(filtered.len(), 1);
    assert_eq!(filtered[0].label, "Sony");
    assert_eq!(filtered[0].profiles, vec![1]);

    assert!(filter_groups(&catalog.brand_groups, &names, "nothing").is_empty());

    // Single-profile brands are collated, so an empty query keeps Sony plus
    // the "Other brands" folder.
    let all = filter_groups(&catalog.brand_groups, &names, "  ");
    assert_eq!(all.len(), 2);
    assert_eq!(all[1].label, "Other brands");
    assert_eq!(all[1].profiles.len(), 2);
}

#[test]
fn grouping_mode_defaults_to_the_flat_list() {
    assert_eq!(Grouping::default(), Grouping::All);
}

#[test]
fn real_library_is_fully_categorised() {
    use rometric::autoeq::AutoEq;

    let db = AutoEq::load().expect("embedded database loads");
    let catalog = db.catalog();

    assert_eq!(catalog.brands.len(), db.len());
    assert_eq!(catalog.signatures.len(), db.len());

    let brand_total: usize = catalog.brand_groups.iter().map(|group| group.len()).sum();
    assert_eq!(brand_total, db.len());
    assert!(
        catalog.brand_groups.len() > 150,
        "expected many brand folders, got {}",
        catalog.brand_groups.len()
    );

    let labels: Vec<&str> = catalog
        .brand_groups
        .iter()
        .filter(|group| !group.is_other)
        .map(|group| group.label.as_str())
        .collect();
    let mut sorted = labels.clone();
    sorted.sort_by_key(|label| label.to_lowercase());
    assert_eq!(labels, sorted, "brand folders must be alphabetical");

    let sony = catalog
        .brand_groups
        .iter()
        .find(|group| group.label == "Sony")
        .expect("Sony folder exists");
    assert!(sony.len() > 100);
    assert!(catalog
        .brand_groups
        .iter()
        .any(|group| group.label == "Bowers & Wilkins"));
    assert_eq!(catalog.brand_groups.last().map(|g| g.is_other), Some(true));

    let signature_total: usize = catalog
        .signature_groups
        .iter()
        .map(|group| group.len())
        .sum();
    assert_eq!(signature_total, db.len());
    assert!(catalog
        .signature_groups
        .iter()
        .any(|group| group.label == "Adds treble"));
}
