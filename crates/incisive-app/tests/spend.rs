//! Spending-cap tests. Pure tests run anywhere; DB tests are gated on
//! `TEST_DATABASE_URL`. Each DB test works in its own far-future IST month
//! (via the `_at` variants) and clears that month first, so tests never
//! share rows and re-runs start clean.

use chrono::{DateTime, TimeZone, Utc};
use claude_client::Usage;
use incisive_app::db::{lazy_pool, migrate};
use incisive_app::identity::{Role, User};
use incisive_app::spend::{
    CapCheck, CapsConfig, Prices, SpendError, cost_paise, default_caps, reserve_at, settle,
    totals_at,
};
use secrecy::SecretString;
use sqlx::PgPool;
use uuid::Uuid;

const PRICES_JSON: &str = r#"{"m-test": {"input": 1.0, "output": 5.0}}"#;

fn prices() -> Prices {
    Prices::from_json(PRICES_JSON).unwrap()
}

fn usage(i: u32, o: u32) -> Usage {
    Usage {
        input_tokens: i,
        output_tokens: o,
    }
}

// ---------- pure ----------

#[test]
fn cost_rounds_up_to_paisa() {
    // 1,000 in + 150 out at $1/$5 per Mtok = $0.00175; at 88 INR/USD = 0.154 INR
    // = 15.4 paise -> rounds UP to 16.
    assert_eq!(
        cost_paise(&prices(), 88.0, "m-test", &usage(1000, 150)).unwrap(),
        16
    );
    // Tiny non-zero usage still costs at least one paisa.
    assert_eq!(
        cost_paise(&prices(), 88.0, "m-test", &usage(1, 0)).unwrap(),
        1
    );
    // Zero usage is free.
    assert_eq!(
        cost_paise(&prices(), 88.0, "m-test", &usage(0, 0)).unwrap(),
        0
    );
    // Exact amounts are not bumped by float noise: 1M in at $1 and 100 INR/USD = 100 INR.
    assert_eq!(
        cost_paise(&prices(), 100.0, "m-test", &usage(1_000_000, 0)).unwrap(),
        10_000
    );
}

#[test]
fn unknown_model_fails_closed() {
    let err = cost_paise(&prices(), 88.0, "mystery-model", &usage(10, 10)).unwrap_err();
    assert_eq!(err, SpendError::UnknownModel("mystery-model".to_string()));
}

#[test]
fn prices_json_parse_errors_name_variable() {
    for bad in [
        "not json",
        "[]",
        r#"{"m": {"input": 1.0}}"#,
        r#"{"m": {"input": "x", "output": 1}}"#,
        r#"{"m": {"input": -1, "output": 1}}"#,
        "{}",
    ] {
        let err = Prices::from_json(bad).unwrap_err();
        assert!(err.to_string().contains("MODEL_PRICES"), "{bad}: {err}");
    }
    assert!(Prices::from_json(PRICES_JSON).is_ok());
}

#[test]
fn default_caps_match_policy() {
    assert_eq!(default_caps(Role::Owner), (150_000, None));
    assert_eq!(default_caps(Role::Surgeon), (30_000, Some(300_000)));
    assert_eq!(default_caps(Role::Fellow), (15_000, Some(150_000)));
}

// ---------- database gated ----------

async fn test_pool() -> Option<PgPool> {
    let Ok(url) = std::env::var("TEST_DATABASE_URL") else {
        eprintln!("SKIP: TEST_DATABASE_URL not set");
        return None;
    };
    let pool = lazy_pool(&SecretString::from(url)).expect("lazy pool");
    migrate(&pool).await.expect("migrate");
    Some(pool)
}

/// Mid-month, mid-day IST instant for a test's private year (month 5, day 15, 12:00 UTC).
fn at(year: i32) -> DateTime<Utc> {
    Utc.with_ymd_and_hms(year, 5, 15, 12, 0, 0).unwrap()
}

/// Remove every spend row in the test's private year so reruns start clean.
async fn clean_year(pool: &PgPool, year: i32) {
    sqlx::query("DELETE FROM incisive.spend WHERE at >= $1 AND at < $2")
        .bind(Utc.with_ymd_and_hms(year, 1, 1, 0, 0, 0).unwrap())
        .bind(Utc.with_ymd_and_hms(year + 1, 1, 1, 0, 0, 0).unwrap())
        .execute(pool)
        .await
        .unwrap();
}

async fn make_user(pool: &PgPool, name: &str, daily: i64, monthly: Option<i64>) -> User {
    let email = format!("spend-{name}@example.com");
    sqlx::query(
        "INSERT INTO incisive.users (email, display_name, role, daily_cap_paise, monthly_cap_paise) \
         VALUES ($1, 'T', 'surgeon', $2, $3) \
         ON CONFLICT (email) DO UPDATE SET daily_cap_paise = $2, monthly_cap_paise = $3",
    )
    .bind(&email)
    .bind(daily)
    .bind(monthly)
    .execute(pool)
    .await
    .unwrap();
    incisive_app::identity::find_user(pool, &email)
        .await
        .unwrap()
        .unwrap()
}

fn big_global() -> CapsConfig {
    CapsConfig {
        global_monthly_paise: 1_000_000_000,
    }
}

fn is_ok(c: &CapCheck) -> bool {
    matches!(c, CapCheck::Ok { .. })
}

#[tokio::test]
async fn blocks_when_daily_cap_would_be_exceeded() {
    let Some(pool) = test_pool().await else {
        return;
    };
    let year = 2051;
    clean_year(&pool, year).await;
    let u = make_user(&pool, "daily", 10_000, Some(1_000_000)).await;
    let now = at(year);

    let a = reserve_at(&pool, &u, &big_global(), 6_000, Uuid::new_v4(), "ask", now)
        .await
        .unwrap();
    assert!(is_ok(&a));
    let b = reserve_at(&pool, &u, &big_global(), 4_001, Uuid::new_v4(), "ask", now)
        .await
        .unwrap();
    match b {
        CapCheck::Blocked { message } => assert_eq!(
            message,
            "You've reached today's Incisive limit (\u{20b9}100). It resets at midnight IST."
        ),
        other => panic!("expected block, got {other:?}"),
    }
    // Exactly reaching the cap is allowed.
    let c = reserve_at(&pool, &u, &big_global(), 4_000, Uuid::new_v4(), "ask", now)
        .await
        .unwrap();
    assert!(is_ok(&c));
    // The blocked request left no row behind.
    let t = totals_at(&pool, u.id, now).await.unwrap();
    assert_eq!(t.today_paise, 10_000);
}

#[tokio::test]
async fn monthly_cap_blocks_with_month_message() {
    let Some(pool) = test_pool().await else {
        return;
    };
    let year = 2052;
    clean_year(&pool, year).await;
    let u = make_user(&pool, "monthly", 100_000, Some(20_000)).await;
    let day1 = Utc.with_ymd_and_hms(year, 5, 14, 12, 0, 0).unwrap();
    let day2 = Utc.with_ymd_and_hms(year, 5, 15, 12, 0, 0).unwrap();
    assert!(is_ok(
        &reserve_at(
            &pool,
            &u,
            &big_global(),
            15_000,
            Uuid::new_v4(),
            "ask",
            day1
        )
        .await
        .unwrap()
    ));
    match reserve_at(&pool, &u, &big_global(), 5_001, Uuid::new_v4(), "ask", day2)
        .await
        .unwrap()
    {
        CapCheck::Blocked { message } => assert_eq!(
            message,
            "You've reached this month's Incisive limit (\u{20b9}200). It resets at midnight IST on the 1st."
        ),
        other => panic!("expected block, got {other:?}"),
    }
}

#[tokio::test]
async fn month_and_day_use_ist_boundaries() {
    let Some(pool) = test_pool().await else {
        return;
    };
    let year = 2053;
    clean_year(&pool, year).await;
    let u = make_user(&pool, "ist", 1_000_000, Some(1_000_000)).await;

    // Same UTC date, 2 minutes apart, straddling IST midnight (18:30 UTC).
    let before = Utc.with_ymd_and_hms(year, 3, 10, 18, 29, 0).unwrap();
    let after = Utc.with_ymd_and_hms(year, 3, 10, 18, 31, 0).unwrap();
    assert!(is_ok(
        &reserve_at(&pool, &u, &big_global(), 100, Uuid::new_v4(), "ask", before)
            .await
            .unwrap()
    ));
    let t_before = totals_at(&pool, u.id, before).await.unwrap();
    let t_after = totals_at(&pool, u.id, after).await.unwrap();
    assert_eq!((t_before.today_paise, t_before.month_paise), (100, 100));
    assert_eq!(t_after.today_paise, 0, "new IST day starts at 18:30 UTC");
    assert_eq!(t_after.month_paise, 100, "same IST month");

    // Month end: 31 Mar 18:29 UTC is still March IST; 18:31 UTC is 1 April IST.
    let m_before = Utc.with_ymd_and_hms(year, 3, 31, 18, 29, 0).unwrap();
    let m_after = Utc.with_ymd_and_hms(year, 3, 31, 18, 31, 0).unwrap();
    assert!(is_ok(
        &reserve_at(
            &pool,
            &u,
            &big_global(),
            50,
            Uuid::new_v4(),
            "ask",
            m_before
        )
        .await
        .unwrap()
    ));
    let mb = totals_at(&pool, u.id, m_before).await.unwrap();
    let ma = totals_at(&pool, u.id, m_after).await.unwrap();
    assert_eq!(mb.month_paise, 150);
    assert_eq!(ma.month_paise, 0, "new IST month starts at 18:30 UTC");
    assert_eq!(ma.today_paise, 0);
}

#[tokio::test]
async fn alert_at_80_percent() {
    let Some(pool) = test_pool().await else {
        return;
    };
    let year = 2054;
    clean_year(&pool, year).await;
    let u = make_user(&pool, "alert", 10_000, None).await;
    let now = at(year);

    match reserve_at(&pool, &u, &big_global(), 7_999, Uuid::new_v4(), "ask", now)
        .await
        .unwrap()
    {
        CapCheck::Ok { alert } => assert_eq!(alert, None),
        other => panic!("{other:?}"),
    }
    match reserve_at(&pool, &u, &big_global(), 1, Uuid::new_v4(), "ask", now)
        .await
        .unwrap()
    {
        CapCheck::Ok { alert } => {
            let a = alert.expect("alert at exactly 80%");
            assert!(a.contains("80%") && a.contains("today"), "{a}");
        }
        other => panic!("{other:?}"),
    }
}

#[tokio::test]
async fn settle_replaces_reservation() {
    let Some(pool) = test_pool().await else {
        return;
    };
    let year = 2055;
    clean_year(&pool, year).await;
    let u = make_user(&pool, "settle", 100_000, None).await;
    let now = Utc::now();
    let rid = Uuid::new_v4();
    assert!(is_ok(
        &reserve_at(&pool, &u, &big_global(), 500, rid, "ask", now)
            .await
            .unwrap()
    ));
    let before = totals_at(&pool, u.id, now).await.unwrap();

    settle(
        &pool,
        u.id,
        rid,
        Some("m-test"),
        Some(&usage(1000, 150)),
        16,
    )
    .await
    .unwrap();
    type ChargeRow = (String, i64, Option<String>, Option<i32>, Option<i32>);
    let rows: Vec<ChargeRow> = sqlx::query_as(
        "SELECT kind, paise, model, input_tokens, output_tokens FROM incisive.spend \
         WHERE request_id = $1",
    )
    .bind(rid)
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(
        rows,
        vec![(
            "charge".to_string(),
            16,
            Some("m-test".to_string()),
            Some(1000),
            Some(150)
        )]
    );
    let after = totals_at(&pool, u.id, now).await.unwrap();
    assert_eq!(after.today_paise, before.today_paise - 500 + 16);

    // No model call happened: zero-paise charge, no model or tokens.
    let rid2 = Uuid::new_v4();
    assert!(is_ok(
        &reserve_at(&pool, &u, &big_global(), 500, rid2, "ask", now)
            .await
            .unwrap()
    ));
    settle(&pool, u.id, rid2, None, None, 0).await.unwrap();
    let rows: Vec<(String, i64, Option<String>)> =
        sqlx::query_as("SELECT kind, paise, model FROM incisive.spend WHERE request_id = $1")
            .bind(rid2)
            .fetch_all(&pool)
            .await
            .unwrap();
    assert_eq!(rows, vec![("charge".to_string(), 0, None)]);
    sqlx::query("DELETE FROM incisive.spend WHERE user_id = $1")
        .bind(u.id)
        .execute(&pool)
        .await
        .unwrap();
}

#[tokio::test]
async fn global_cap_blocks_everyone() {
    let Some(pool) = test_pool().await else {
        return;
    };
    let year = 2056;
    clean_year(&pool, year).await;
    let a = make_user(&pool, "global-a", 1_000_000, None).await;
    let b = make_user(&pool, "global-b", 1_000_000, None).await;
    let caps = CapsConfig {
        global_monthly_paise: 10_000,
    };
    let now = at(year);
    assert!(is_ok(
        &reserve_at(&pool, &a, &caps, 7_000, Uuid::new_v4(), "ask", now)
            .await
            .unwrap()
    ));
    // A different user is blocked once the shared budget would be exceeded.
    match reserve_at(&pool, &b, &caps, 3_001, Uuid::new_v4(), "ask", now)
        .await
        .unwrap()
    {
        CapCheck::Blocked { message } => assert_eq!(
            message,
            "Incisive has reached its overall monthly budget. Please tell the owner."
        ),
        other => panic!("expected block, got {other:?}"),
    }
    assert!(is_ok(
        &reserve_at(&pool, &b, &caps, 3_000, Uuid::new_v4(), "ask", now)
            .await
            .unwrap()
    ));
}

#[tokio::test]
async fn concurrent_reservations_do_not_overshoot() {
    let Some(pool) = test_pool().await else {
        return;
    };
    let year = 2057;
    clean_year(&pool, year).await;
    let u = make_user(&pool, "race", 10_000, None).await;
    let now = at(year);
    for round in 0..5 {
        // Fresh day each round so rounds are independent.
        let now = now + chrono::Duration::days(round);
        let (p1, p2, u1, u2) = (pool.clone(), pool.clone(), u.clone(), u.clone());
        let t1 = tokio::spawn(async move {
            reserve_at(&p1, &u1, &big_global(), 6_000, Uuid::new_v4(), "ask", now).await
        });
        let t2 = tokio::spawn(async move {
            reserve_at(&p2, &u2, &big_global(), 6_000, Uuid::new_v4(), "ask", now).await
        });
        let (r1, r2) = (t1.await.unwrap().unwrap(), t2.await.unwrap().unwrap());
        assert_eq!(
            [&r1, &r2].iter().filter(|c| is_ok(c)).count(),
            1,
            "exactly one of two 60% reservations may pass: {r1:?} {r2:?}"
        );
        let t = totals_at(&pool, u.id, now).await.unwrap();
        assert_eq!(t.today_paise, 6_000);
    }
}

#[tokio::test]
async fn totals_cover_reservations_and_charges_for_current_periods() {
    let Some(pool) = test_pool().await else {
        return;
    };
    let year = 2058;
    clean_year(&pool, year).await;
    let u = make_user(&pool, "totals", 1_000_000, None).await;
    let other = make_user(&pool, "totals-other", 1_000_000, None).await;
    let now = at(year);
    let earlier = now - chrono::Duration::days(3);

    let rid = Uuid::new_v4();
    reserve_at(&pool, &u, &big_global(), 300, rid, "ask", earlier)
        .await
        .unwrap();
    reserve_at(&pool, &u, &big_global(), 200, Uuid::new_v4(), "ask", now)
        .await
        .unwrap();
    reserve_at(
        &pool,
        &other,
        &big_global(),
        999,
        Uuid::new_v4(),
        "ask",
        now,
    )
    .await
    .unwrap();
    let t = totals_at(&pool, u.id, now).await.unwrap();
    assert_eq!((t.today_paise, t.month_paise), (200, 500));
}

#[tokio::test]
async fn totals_now_wrapper_sees_fresh_reservation() {
    let Some(pool) = test_pool().await else {
        return;
    };
    let u = make_user(&pool, "totals-now", 1_000_000, None).await;
    sqlx::query("DELETE FROM incisive.spend WHERE user_id = $1")
        .bind(u.id)
        .execute(&pool)
        .await
        .unwrap();
    let caps = big_global();
    let out = incisive_app::spend::reserve(&pool, &u, &caps, 123, Uuid::new_v4(), "ask")
        .await
        .unwrap();
    assert!(is_ok(&out));
    let t = incisive_app::spend::totals(&pool, u.id).await.unwrap();
    assert_eq!((t.today_paise, t.month_paise), (123, 123));
    sqlx::query("DELETE FROM incisive.spend WHERE user_id = $1")
        .bind(u.id)
        .execute(&pool)
        .await
        .unwrap();
}
