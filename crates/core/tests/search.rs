//! 匹配引擎的验收样例。组内排序不在这里定。

use std::cmp::Ordering;
use std::path::Path;

use lanwork_core::search::{
    CHARACTER_COUNT, FieldInput, FieldRole, GroupOrder, Hit, HitKind, MatchIndex,
    PendingGroupOrder, READINGS_SHA256, SYLLABLE_COUNT, TABLE_RESIDENT_BYTES, UNICODE_VERSION,
    benchmark_corpus, prepare, query_prepared, readings, resident_bytes, sort_hits, table_info,
};
use sha2::{Digest, Sha256};

fn name(text: &str) -> FieldInput<'_> {
    FieldInput {
        role: FieldRole::Name,
        text,
    }
}

fn index_of(text: &str) -> MatchIndex {
    let mut index = MatchIndex::new();
    index.insert(1, &[name(text)]);
    index
}

fn kinds(hits: &[Hit]) -> Vec<HitKind> {
    hits.iter().map(|hit| hit.kind).collect()
}

fn has(hits: &[Hit], kind: HitKind) -> bool {
    hits.iter().any(|hit| hit.kind == kind)
}

#[test]
fn wechat_matches_initials_pinyin_and_han_substring() {
    let index = index_of("微信");
    let wx = index.query("wx");
    assert_eq!(kinds(&wx), vec![HitKind::Initial]);

    let weixin = index.query("weixin");
    assert_eq!(kinds(&weixin), vec![HitKind::Pinyin]);

    let wei = index.query("微");
    assert_eq!(kinds(&wei), vec![HitKind::Prefix]);
    assert_eq!(kinds(&index.query("信")), vec![HitKind::Substring]);
}

#[test]
fn visual_studio_code_matches_vsc_as_initials_and_fuzzy() {
    let index = index_of("Visual Studio Code");
    let hits = index.query("vsc");
    assert!(has(&hits, HitKind::Initial));
    assert!(has(&hits, HitKind::Fuzzy));
    assert!(!has(&hits, HitKind::Pinyin));
    assert!(!has(&hits, HitKind::Substring));
}

#[test]
fn zhong_matches_both_zhong_and_chong_and_keeps_tong() {
    assert_eq!(readings('重'), vec!["zhong", "chong", "tong"]);
    let index = index_of("重");
    assert!(has(&index.query("zhong"), HitKind::Pinyin));
    assert!(has(&index.query("chong"), HitKind::Pinyin));
    assert!(has(&index.query("tong"), HitKind::Pinyin));
    let zh = index.query("zh");
    assert!(has(&zh, HitKind::Pinyin));
    assert!(!has(&zh, HitKind::Initial));
}

#[test]
fn xing_matches_xing_hang_and_keeps_heng() {
    assert_eq!(readings('行'), vec!["xing", "hang", "heng"]);
    let index = index_of("行");
    assert!(has(&index.query("xing"), HitKind::Pinyin));
    assert!(has(&index.query("hang"), HitKind::Pinyin));
    assert!(has(&index.query("heng"), HitKind::Pinyin));
}

#[test]
fn chongqing_matches_without_expanding_combinations() {
    let prepared = prepare(1, &[name("重庆")]);
    assert_eq!(readings('重').len() + readings('庆').len(), 4);
    assert_eq!(prepared.stored_syllable_ids(), 4);
    let hits = query_prepared(std::slice::from_ref(&prepared), "chongqing");
    assert!(has(&hits, HitKind::Pinyin));
    let other = query_prepared(std::slice::from_ref(&prepared), "zhongqing");
    assert!(has(&other, HitKind::Pinyin));
    let tong = query_prepared(std::slice::from_ref(&prepared), "tongqing");
    assert!(has(&tong, HitKind::Pinyin));
}

#[test]
fn repeated_polyphonic_character_does_not_store_the_cartesian_product() {
    let readings_of = readings('\u{64D6}');
    assert!(readings_of.len() >= 2);
    let title = "\u{64D6}".repeat(12);
    let prepared = prepare(7, &[name(&title)]);
    assert_eq!(prepared.stored_syllable_ids(), 12 * readings_of.len());
    let first: String = readings_of[0].repeat(12);
    let last: String = readings_of[readings_of.len() - 1].repeat(12);
    assert!(has(
        &query_prepared(std::slice::from_ref(&prepared), &first),
        HitKind::Pinyin
    ));
    assert!(has(
        &query_prepared(std::slice::from_ref(&prepared), &last),
        HitKind::Pinyin
    ));
}

#[test]
fn fullwidth_and_mixed_case_match() {
    let index = index_of("微信");
    assert!(has(&index.query("ＷＸ"), HitKind::Initial));
    assert!(has(&index.query("WeiXin"), HitKind::Pinyin));
    assert!(has(&index.query("ｗｅｉｘｉｎ"), HitKind::Pinyin));

    let code = index_of("Visual Studio Code");
    assert!(has(&code.query("VSC"), HitKind::Initial));
    assert!(has(&code.query("vSc"), HitKind::Initial));
    assert!(has(&code.query("ＶＳＣ"), HitKind::Initial));
    assert_eq!(
        kinds(&code.query("visual")),
        vec![HitKind::Prefix, HitKind::Fuzzy]
    );
}

#[test]
fn empty_and_whitespace_queries_return_nothing() {
    let index = index_of("微信");
    assert!(index.query("").is_empty());
    assert!(index.query("   ").is_empty());
    assert!(index.query("\t").is_empty());
    assert!(index.query("\u{3000}").is_empty());
    assert!(MatchIndex::new().query("wx").is_empty());
}

#[test]
fn rare_character_outside_the_table_matches_only_the_original_text() {
    assert!(readings('\u{20000}').is_empty());
    let rare = "\u{20000}";
    let index = index_of(rare);
    assert_eq!(kinds(&index.query(rare)), vec![HitKind::Exact]);
    assert!(index.query("zhong").is_empty());
    assert!(index.query("wx").is_empty());

    let mixed = index_of("微\u{20000}信");
    assert!(has(&mixed.query("微"), HitKind::Prefix));
    assert!(has(&mixed.query("wei"), HitKind::Pinyin));
    assert!(has(&mixed.query("xin"), HitKind::Pinyin));
    assert!(mixed.query("weixin").is_empty());
}

#[test]
fn missing_reading_does_not_error_on_emoji_or_long_ascii() {
    let index = index_of("微信");
    assert!(index.query("😀").is_empty());
    assert!(index.query(&"a".repeat(1_000)).is_empty());
    assert!(index.query("zhòng").is_empty());
}

#[test]
fn note_body_is_literal_and_tags_use_pinyin() {
    let mut index = MatchIndex::new();
    index.insert(
        3,
        &[
            name("备忘"),
            FieldInput {
                role: FieldRole::Tag,
                text: "工作",
            },
            FieldInput {
                role: FieldRole::Body,
                text: "明天去微信开会",
            },
        ],
    );
    let body = index.query("微信");
    assert_eq!(body.len(), 1);
    assert_eq!(body[0].role, FieldRole::Body);
    assert_eq!(body[0].kind, HitKind::Substring);

    assert!(index.query("weixin").is_empty());
    let tag = index.query("gongzuo");
    assert!(
        tag.iter()
            .any(|hit| hit.role == FieldRole::Tag && hit.kind == HitKind::Pinyin)
    );
    assert!(
        index
            .query("gz")
            .iter()
            .any(|hit| hit.role == FieldRole::Tag && hit.kind == HitKind::Initial)
    );
}

#[test]
fn alias_uses_the_same_matcher_and_keeps_its_index() {
    let mut index = MatchIndex::new();
    index.insert(
        4,
        &[
            name("微信"),
            FieldInput {
                role: FieldRole::Alias,
                text: "WeChat",
            },
            FieldInput {
                role: FieldRole::Alias,
                text: "微信电脑版",
            },
        ],
    );
    let wechat = index.query("wechat");
    assert!(wechat.iter().any(|hit| {
        hit.role == FieldRole::Alias && hit.field_index == 0 && hit.kind == HitKind::Exact
    }));
    let pinyin = index.query("weixindiannaoban");
    assert!(pinyin.iter().any(|hit| {
        hit.role == FieldRole::Alias && hit.field_index == 1 && hit.kind == HitKind::Pinyin
    }));
    assert!(has(&index.query("wx"), HitKind::Initial));
}

#[test]
fn compact_form_matches_visual_studio_code_without_spaces() {
    let index = index_of("Visual Studio Code");
    assert!(has(&index.query("visualstudiocode"), HitKind::Exact));
    assert!(has(&index.query("visual studio"), HitKind::Prefix));
}

#[test]
fn pinyin_can_start_mid_name_and_rejects_a_broken_syllable() {
    let index = index_of("回复微信消息");
    assert!(has(&index.query("weixin"), HitKind::Pinyin));
    assert!(has(&index.query("wei"), HitKind::Pinyin));
    assert!(has(&index.query("weix"), HitKind::Pinyin));
    assert!(index.query("wexin").is_empty());
    assert_eq!(readings('信'), vec!["xin", "shen"]);
    assert!(has(&index_of("微信").query("weishen"), HitKind::Pinyin));
}

#[test]
fn punctuation_does_not_block_pinyin_and_kana_does() {
    assert!(has(&index_of("微，信").query("weixin"), HitKind::Pinyin));
    let kana = index_of("微あ信");
    assert!(kana.query("weixin").is_empty());
    assert!(has(&kana.query("wei"), HitKind::Pinyin));
    assert!(has(&kana.query("xin"), HitKind::Pinyin));
}

#[test]
fn u_with_diaeresis_is_folded_to_v() {
    assert_eq!(readings('女'), vec!["nv", "ru"]);
    assert!(has(&index_of("女").query("nv"), HitKind::Pinyin));
    assert_eq!(readings('绿'), vec!["lv"]);
    assert!(has(&index_of("绿").query("lv"), HitKind::Pinyin));
}

#[test]
fn accented_latin_is_not_transliterated() {
    let index = index_of("café");
    assert!(has(&index.query("café"), HitKind::Exact));
    assert!(index.query("cafe").is_empty());
}

#[test]
fn fuzzy_scores_reward_consecutive_hits_and_word_starts() {
    let chrome = index_of("chrome").query("chr");
    let catcher = index_of("catcher").query("chr");
    let chrome_score = chrome
        .iter()
        .find(|hit| hit.kind == HitKind::Fuzzy)
        .unwrap()
        .score;
    let catcher_score = catcher
        .iter()
        .find(|hit| hit.kind == HitKind::Fuzzy)
        .unwrap()
        .score;
    assert_eq!(chrome_score, 9);
    assert_eq!(catcher_score, 5);
    assert!(chrome_score > catcher_score);

    let words = index_of("Visual Studio Code").query("vsc");
    let mid = index_of("avbsxc").query("vsc");
    let word_score = words
        .iter()
        .find(|hit| hit.kind == HitKind::Fuzzy)
        .unwrap()
        .score;
    let mid_score = mid
        .iter()
        .find(|hit| hit.kind == HitKind::Fuzzy)
        .unwrap()
        .score;
    assert_eq!(word_score, 9);
    assert_eq!(mid_score, 3);
    assert!(word_score > mid_score);
}

#[test]
fn pending_group_order_keeps_input_order() {
    let mut index = MatchIndex::new();
    index.insert(10, &[name("catcher")]);
    index.insert(11, &[name("chrome")]);
    let mut hits = index.query("chr");
    let fuzzy: Vec<_> = hits
        .iter()
        .filter(|hit| hit.kind == HitKind::Fuzzy)
        .copied()
        .collect();
    assert_eq!(fuzzy[0].id, 10);
    assert_eq!(fuzzy[1].id, 11);
    assert!(fuzzy[0].score < fuzzy[1].score);
    sort_hits(&mut hits, &PendingGroupOrder);
    let after: Vec<_> = hits
        .iter()
        .filter(|hit| hit.kind == HitKind::Fuzzy)
        .map(|hit| hit.id)
        .collect();
    assert_eq!(after, vec![10, 11]);
}

struct ScoreOrder;

impl GroupOrder for ScoreOrder {
    fn cmp(&self, left: &Hit, right: &Hit) -> Ordering {
        right.score.cmp(&left.score).then(left.id.cmp(&right.id))
    }
}

#[test]
fn group_order_trait_can_reorder_without_being_the_default() {
    let mut index = MatchIndex::new();
    index.insert(10, &[name("catcher")]);
    index.insert(11, &[name("chrome")]);
    let mut hits = index.query("chr");
    sort_hits(&mut hits, &ScoreOrder);
    let fuzzy: Vec<_> = hits
        .iter()
        .filter(|hit| hit.kind == HitKind::Fuzzy)
        .map(|hit| hit.id)
        .collect();
    assert_eq!(fuzzy, vec![11, 10]);
}

#[test]
fn same_matcher_covers_an_app_name_and_a_todo_title() {
    let mut index = MatchIndex::new();
    index.insert(1, &[name("微信")]);
    index.insert(2, &[name("给微信回复")]);
    let hits = index.query("weixin");
    assert_eq!(hits.len(), 2);
    assert!(hits.iter().all(|hit| hit.kind == HitKind::Pinyin));
    assert_eq!(hits[0].id, 1);
    assert_eq!(hits[1].id, 2);
}

#[test]
fn unihan_file_matches_the_pinned_checksum_and_license_is_present() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../third_party/unihan");
    let bytes = std::fs::read(root.join("kMandarin_kHanyuPinyin.txt")).unwrap();
    let digest = hex_encode(&Sha256::digest(bytes));
    assert_eq!(digest, READINGS_SHA256);
    let license = std::fs::read_to_string(root.join("LICENSE.txt")).unwrap();
    assert!(license.contains("UNICODE LICENSE V3"));
    let readme = std::fs::read_to_string(root.join("README.md")).unwrap();
    assert!(readme.contains(READINGS_SHA256));
    assert!(readme.contains(UNICODE_VERSION));
    let info = table_info();
    assert_eq!(info.unicode_version, "18.0.0");
    assert_eq!(info.character_count, CHARACTER_COUNT);
    assert_eq!(info.syllable_count, SYLLABLE_COUNT);
    assert_eq!(info.resident_bytes, resident_bytes());
    assert_eq!(resident_bytes(), TABLE_RESIDENT_BYTES);
    assert_eq!(resident_bytes(), 207_860);
    assert_eq!(CHARACTER_COUNT, 26_711);
    assert_eq!(SYLLABLE_COUNT, 419);
    assert!(resident_bytes() < 512 * 1024);
}

#[test]
fn benchmark_corpus_covers_the_acceptance_names_and_stays_small() {
    let index = benchmark_corpus(5_000, 10_000);
    assert_eq!(index.len(), 15_000);
    assert!(
        index
            .query("wx")
            .iter()
            .any(|hit| hit.id == 0 && hit.kind == HitKind::Initial)
    );
    assert!(
        index
            .query("vsc")
            .iter()
            .any(|hit| hit.id == 1 && hit.kind == HitKind::Initial)
    );
    assert!(
        index
            .query("weixin")
            .iter()
            .any(|hit| hit.id == 5_000 && hit.kind == HitKind::Pinyin)
    );
    let heap = index.heap_bytes();
    assert!(heap < 16 * 1024 * 1024, "corpus heap bytes {heap}");
    assert!(heap > 0);
}

fn hex_encode(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(HEX[(byte >> 4) as usize] as char);
        out.push(HEX[(byte & 0x0f) as usize] as char);
    }
    out
}
