//! Rebuilt layouts against every type tree a real install ships. Files only.

use std::collections::BTreeMap;
use unity_files::archive::Archive;
use unity_files::il2cpp::Il2Cpp;
use unity_files::layout::{Layouts, difference};
use unity_files::serialized::MONO_BEHAVIOUR;
use unity_files::value::Value;

#[test]
#[ignore = "compares with a Last Epoch install named by EG_LAST_EPOCH"]
fn generated_layouts_match_the_shipped_type_trees() {
    let install = crate::Install::of(std::path::Path::new(&std::env::var_os("EG_LAST_EPOCH").expect("EG_LAST_EPOCH")).join("Last Epoch.exe")).unwrap();
    let il2cpp = Il2Cpp::open(&install.game_assembly(), &install.metadata()).unwrap();
    let layouts = Layouts::new(&il2cpp);
    let mut archive = Archive::open(install.definition_sources()).unwrap();
    let file = archive.main_file(0).unwrap();
    let mut classes = BTreeMap::new();
    for object in file.objects().iter().filter(|o| o.class_id == MONO_BEHAVIOUR) {
        let head = file.decode_prefix(object, "m_Name").unwrap();
        if head.field("m_Script", Value::as_pptr).unwrap() == (0, 0) {
            continue;
        }
        let script = archive.script(&file, &head).unwrap();
        classes
            .entry((script.assembly.clone(), script.namespace.clone(), script.class.clone()))
            .or_insert_with(|| file.tree(object).unwrap().clone());
    }
    let (mut matched, mut failures) = (0, BTreeMap::new());
    for ((assembly, namespace, class), real) in &classes {
        let outcome = match layouts.mono_behaviour(assembly, namespace, class) {
            Ok(made) => difference(real, &made),
            Err(e) => Some(format!("not generated: {e}")),
        };
        match outcome {
            None => matched += 1,
            Some(why) => {
                failures.insert(format!("{assembly}:{namespace}.{class}"), why);
            }
        }
    }
    let mut reasons: BTreeMap<String, usize> = BTreeMap::new();
    for why in failures.values() {
        let kind = why.split(':').next_back().unwrap_or(why).trim().chars().take(70).collect::<String>();
        *reasons.entry(kind).or_default() += 1;
    }
    println!("{matched} of {} classes match", classes.len());
    for (class, why) in failures.iter().filter(|(_, w)| !w.contains("SerializeReference")).take(40) {
        println!("  {class}: {why}");
    }
    let mut common: Vec<_> = reasons.into_iter().collect();
    common.sort_by_key(|(_, count)| std::cmp::Reverse(*count));
    for (why, count) in common.iter().take(15) {
        println!("{count:5} × {why}");
    }
    // Classes with [SerializeReference] fields are refused by design; every other class must match exactly.
    let mismatched: Vec<_> = failures.iter().filter(|(_, why)| !why.contains("[SerializeReference]")).collect();
    assert!(mismatched.is_empty(), "{} classes differ: {mismatched:?}", mismatched.len());
    assert!(matched > 1000, "only {matched} classes compared");
}
