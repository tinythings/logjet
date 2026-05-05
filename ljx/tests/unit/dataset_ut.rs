use std::path::PathBuf;

use crate::dataset::Dataset;
use crate::error::Error;

#[test]
fn dataset_sorts_and_dedups_paths() {
    let dir = std::env::temp_dir().join(format!("ljx-dataset-ut-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let tatooine = dir.join("tatooine.logjet");
    let alderaan = dir.join("alderaan.logjet");
    std::fs::write(&tatooine, b"binary-sand").unwrap();
    std::fs::write(&alderaan, b"binary-stars").unwrap();

    let ds = Dataset::from_inputs(&[tatooine.clone(), alderaan.clone(), tatooine.clone()]).unwrap();
    let paths = ds.entries().iter().map(|entry| entry.path.clone()).collect::<Vec<_>>();
    assert_eq!(paths, vec![alderaan.clone(), tatooine.clone()]);
    assert_eq!(ds.entries()[0].size, 12);
    assert!(ds.entries()[0].modified_ns.is_some());

    let _ = std::fs::remove_file(tatooine);
    let _ = std::fs::remove_file(alderaan);
    let _ = std::fs::remove_dir(dir);
}

#[test]
fn dataset_expands_directories_into_logjet_files() {
    let dir = std::env::temp_dir().join(format!("ljx-dataset-dir-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let yavin = dir.join("yavin.logjet");
    let notes = dir.join("notes.txt");
    std::fs::write(&yavin, b"death-star-plans").unwrap();
    std::fs::write(&notes, b"ignore-me").unwrap();

    let ds = Dataset::from_inputs(std::slice::from_ref(&dir)).unwrap();
    assert_eq!(ds.len(), 1);
    assert_eq!(ds.entries()[0].path, yavin);

    let _ = std::fs::remove_file(notes);
    let _ = std::fs::remove_file(ds.entries()[0].path.clone());
    let _ = std::fs::remove_dir(dir);
}

#[test]
fn dataset_walks_subdirectories_for_logjet_files() {
    let dir = std::env::temp_dir().join(format!("ljx-dataset-tree-{}", std::process::id()));
    let rebels = dir.join("rebels");
    let empire = dir.join("empire");
    std::fs::create_dir_all(&rebels).unwrap();
    std::fs::create_dir_all(&empire).unwrap();
    let yavin = rebels.join("yavin.logjet");
    let hoth = rebels.join("hoth.logjet");
    let endor = empire.join("endor.logjet");
    std::fs::write(&yavin, b"one").unwrap();
    std::fs::write(&hoth, b"two").unwrap();
    std::fs::write(&endor, b"three").unwrap();

    let ds = Dataset::from_inputs(std::slice::from_ref(&dir)).unwrap();
    let paths = ds.entries().iter().map(|entry| entry.path.clone()).collect::<Vec<_>>();
    assert_eq!(paths, vec![endor.clone(), hoth.clone(), yavin.clone()]);

    let _ = std::fs::remove_file(yavin);
    let _ = std::fs::remove_file(hoth);
    let _ = std::fs::remove_file(endor);
    let _ = std::fs::remove_dir(rebels);
    let _ = std::fs::remove_dir(empire);
    let _ = std::fs::remove_dir(dir);
}

#[test]
fn dataset_rejects_unexpanded_glob() {
    let err = Dataset::from_inputs(&[PathBuf::from("rebels/*.logjet")]).unwrap_err();
    assert!(matches!(err, Error::Usage(msg) if msg.contains("expects the shell to expand globs")));
}
