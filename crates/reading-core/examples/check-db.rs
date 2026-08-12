//! Exercise the Postgres layer against a real database.
//!
//!     cargo run -p reading-core --example check-db
//!
//! Reads DATABASE_URL from .env. Creates two throwaway users and proves the
//! thing that actually matters: one cannot see the other's books. Cleans up
//! after itself, so it is safe to run repeatedly against a dev database.
//!
//! This is a diagnostic rather than a test because it needs a live server.
//! `cargo test` has to pass on a fresh clone with no Postgres anywhere.

use reading_core::db::pg::{Db, UserId};
use reading_core::models::{BlockKind, DraftBlock};
use reading_core::sqlx::Row;

#[tokio::main]
async fn main() {
    dotenvy::dotenv().ok();

    let url = match std::env::var("DATABASE_URL") {
        Ok(u) => u,
        Err(_) => {
            eprintln!("DATABASE_URL is not set. Run scripts/setup-postgres.ps1 first.");
            std::process::exit(1);
        }
    };

    // Printed without the password, so this can go in a bug report.
    println!("Connecting to {}\n", redact(&url));

    let db = match Db::connect(&url).await {
        Ok(db) => db,
        Err(e) => {
            eprintln!("Could not connect: {e}");
            std::process::exit(1);
        }
    };
    println!("connected, migrations applied");

    if let Err(e) = run(&db).await {
        eprintln!("\nFAILED: {e}");
        std::process::exit(1);
    }
}

async fn run(db: &Db) -> reading_core::Result<()> {
    // Distinct per run so a previous crash cannot collide with this one.
    let tag = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis();

    let first_ever = !db.has_any_user().await?;
    println!("first user would be an admin: {first_ever}");

    println!("\n--- users ---");
    let alice = db
        .create_user(
            &format!("alice-{tag}@example.test"),
            "Alice",
            "$argon2id$placeholder",
            false,
        )
        .await?;
    let bob = db
        .create_user(
            &format!("bob-{tag}@example.test"),
            "Bob",
            "$argon2id$placeholder",
            false,
        )
        .await?;
    println!("created {} and {}", alice.get(), bob.get());

    // The unique index is on lower(email), so case must not create a second
    // account for the same person.
    let duplicate = db
        .create_user(
            &format!("ALICE-{tag}@EXAMPLE.TEST"),
            "Impostor",
            "$argon2id$placeholder",
            false,
        )
        .await;
    match duplicate {
        Err(e) => println!("same address in caps rejected: {e}"),
        Ok(_) => return Err(oops("a duplicate email was accepted")),
    }

    println!("\n--- books ---");
    let alices_book = db
        .create_book(alice, "Systematic Theology", Some("Charles Hodge"), "victorian")
        .await?;
    let bobs_book = db.create_book(bob, "Emma", Some("Jane Austen"), "victorian").await?;
    println!("alice's book {alices_book}, bob's book {bobs_book}");

    let listed = db.list_books(alice).await?;
    if listed.len() != 1 || listed[0].id != alices_book {
        return Err(oops("list_books returned the wrong set"));
    }
    println!("alice sees exactly her own book: {:?}", listed[0].title);

    println!("\n--- isolation ---");
    // The whole point. Bob's id against Alice's session must not resolve.
    match db.get_book(alice, bobs_book).await {
        Err(e) => println!("alice cannot read bob's book: {e}"),
        Ok(b) => return Err(oops(&format!("LEAK: alice read bob's book {:?}", b.title))),
    }

    match db.delete_book(alice, bobs_book).await {
        Err(e) => println!("alice cannot delete bob's book: {e}"),
        Ok(_) => return Err(oops("LEAK: alice deleted bob's book")),
    }

    if db.get_book(bob, bobs_book).await.is_err() {
        return Err(oops("bob's book vanished, so the failed delete was not a no-op"));
    }
    println!("bob's book is still there");

    println!("\n--- resume ---");
    // No pages yet, so there is nowhere to resume to.
    let resume = db.resume_page(alice, alices_book).await?;
    println!("resume page with no pages: {resume:?}");

    // A page id that is not in this book must be refused, even from its owner.
    match db.set_last_page(alice, alices_book, Some(999_999)).await {
        Err(e) => println!("a page from another book is refused: {e}"),
        Ok(_) => return Err(oops("set_last_page accepted a foreign page")),
    }

    println!("\n--- pages and blocks ---");
    let draft = |text: &str| DraftBlock {
        kind: BlockKind::Paragraph,
        text_raw: text.to_string(),
        text_norm: text.to_string(),
    };

    let page_two = db
        .add_text_page(
            alice,
            alices_book,
            "epub",
            Some("ch1"),
            "hodge.epub",
            Some(2),
            &[draft("Theology is a science."), draft("It has a method.")],
        )
        .await?;
    let page_one = db
        .add_text_page(
            alice,
            alices_book,
            "epub",
            Some("ch0"),
            "hodge.epub",
            Some(1),
            &[draft("On method.")],
        )
        .await?;
    println!("added pages {page_one} and {page_two}, out of order on purpose");

    let pages = db.list_pages(alice, alices_book).await?;
    let order: Vec<Option<i64>> = pages.iter().map(|p| p.page_no).collect();
    if order != vec![Some(1), Some(2)] {
        return Err(oops(&format!("pages came back in the wrong order: {order:?}")));
    }
    println!("listed in printed-page order, not import order: {order:?}");

    let blocks = db.list_blocks(alice, page_two).await?;
    if blocks.len() != 2 || blocks[0].ordinal != 0 || blocks[1].ordinal != 1 {
        return Err(oops("blocks did not come back in ordinal order"));
    }
    println!("page {page_two} has {} blocks in order", blocks.len());

    // Adjacency follows the printed number, not the order of import.
    let next = db.next_page_id(alice, page_one).await?;
    let prev = db.previous_page_id(alice, page_two).await?;
    if next != Some(page_two) || prev != Some(page_one) {
        return Err(oops(&format!("adjacency is wrong: next={next:?} prev={prev:?}")));
    }
    println!("page 1 -> page 2 -> back again, by printed number");

    if db.next_page_id(alice, page_two).await?.is_some() {
        return Err(oops("found a page after the last one"));
    }
    println!("nothing after the last page");

    println!("\n--- isolation, deeper ---");
    let block_id = blocks[0].id;
    match db.get_block(bob, block_id).await {
        Err(e) => println!("bob cannot read a block of alice's: {e}"),
        Ok(_) => return Err(oops("LEAK: bob read alice's block")),
    }
    match db.edit_block(bob, block_id, "vandalised").await {
        Err(e) => println!("bob cannot edit it: {e}"),
        Ok(_) => return Err(oops("LEAK: bob edited alice's block")),
    }
    match db.delete_block(bob, block_id).await {
        Err(e) => println!("bob cannot delete it: {e}"),
        Ok(_) => return Err(oops("LEAK: bob deleted alice's block")),
    }
    match db.list_blocks(bob, page_two).await {
        Ok(v) if v.is_empty() => println!("bob lists alice's page as empty"),
        Ok(_) => return Err(oops("LEAK: bob listed alice's blocks")),
        Err(e) => println!("bob cannot list alice's blocks: {e}"),
    }
    match db.delete_page(bob, page_two).await {
        Err(e) => println!("bob cannot delete alice's page: {e}"),
        Ok(_) => return Err(oops("LEAK: bob deleted alice's page")),
    }

    // And none of those refusals damaged anything.
    let after = db.get_block(alice, block_id).await?;
    if after.text_norm != "Theology is a science." || after.user_edited {
        return Err(oops("the block was changed by a refused write"));
    }
    println!("alice's block is untouched after all of that");

    println!("\n--- edits ---");
    db.edit_block(alice, block_id, "Theology is a science, properly so called.")
        .await?;
    let edited = db.get_block(alice, block_id).await?;
    if !edited.user_edited {
        return Err(oops("edit_block did not set user_edited"));
    }
    println!("edit records that a human touched it: {:?}", edited.text_norm);

    match db.set_block_kind(alice, block_id, "marginalia").await {
        Err(e) => println!("an invented block kind is refused: {e}"),
        Ok(_) => return Err(oops("set_block_kind accepted a bad kind")),
    }
    db.set_block_kind(alice, block_id, "heading").await?;
    println!("a real one is accepted");

    println!("\n--- stats ---");
    let stats = db.book_stats(alice, alices_book).await?;
    println!(
        "{} pages, {} paragraphs, {} summarised, {} words",
        stats.pages, stats.paragraphs, stats.summaries, stats.words_looked_up
    );
    if stats.pages != 2 {
        return Err(oops("book_stats counted the wrong number of pages"));
    }
    match db.book_stats(bob, alices_book).await {
        Err(_) => println!("bob gets no statistics for alice's book"),
        Ok(_) => return Err(oops("LEAK: bob read alice's book statistics")),
    }

    println!("\n--- summaries ---");
    let prose = blocks[1].id;
    let first = db
        .save_summary(alice, prose, None, "It has a method.", false)
        .await?;
    let second = db
        .save_summary(alice, prose, None, "Theology proceeds by a method.", true)
        .await?;
    let latest = db
        .latest_summary(alice, prose)
        .await?
        .ok_or_else(|| oops("no summary came back"))?;
    if latest.revision != 2 || latest.id != second || !latest.self_checked {
        return Err(oops(&format!("wrong revision came back: {latest:?}")));
    }
    println!("two revisions, latest is #{} ({first} then {second})", latest.revision);

    // Sentence revisions are counted per sentence, so writing about sentence 0
    // must not bump the paragraph's revision number.
    db.save_summary(alice, prose, Some(0), "A method exists.", false)
        .await?;
    db.save_summary(alice, prose, Some(0), "There is a method.", false)
        .await?;
    db.save_summary(alice, prose, Some(1), "It is applied.", false)
        .await?;

    let notes = db.sentence_summaries(alice, prose).await?;
    if notes.len() != 2 || notes[0].ordinal != 0 || notes[0].text != "There is a method." {
        return Err(oops(&format!("sentence notes are wrong: {notes:?}")));
    }
    println!("latest note per sentence, in order: {:?}", notes.iter().map(|n| &n.text).collect::<Vec<_>>());

    let still = db.latest_summary(alice, prose).await?.unwrap();
    if still.revision != 2 {
        return Err(oops("a sentence note bumped the paragraph revision"));
    }
    println!("paragraph revision unaffected by sentence notes: still #{}", still.revision);

    println!("\n--- critiques ---");
    db.save_critique(
        alice,
        second,
        "partial",
        (true, false, false),
        "vague",
        "What does the method do?",
        "qwen3:4b",
    )
    .await?;
    println!("critique stored against the summary");

    match db
        .save_critique(bob, second, "on_target", (true, true, true), "none", "", "qwen3:4b")
        .await
    {
        Err(e) => println!("bob cannot critique alice's summary: {e}"),
        Ok(_) => return Err(oops("LEAK: bob wrote a critique on alice's summary")),
    }
    match db.save_summary(bob, prose, None, "hijacked", false).await {
        Err(e) => println!("bob cannot summarise alice's block: {e}"),
        Ok(_) => return Err(oops("LEAK: bob summarised alice's block")),
    }

    println!("\n--- spine ---");
    let spine = db.summary_spine(alice, alices_book).await?;
    if spine.len() != 1 || spine[0].sentence != "Theology proceeds by a method." {
        return Err(oops(&format!("spine is wrong: {spine:?}")));
    }
    println!("spine has {} entry, the latest revision only", spine.len());
    if !db.summary_spine(bob, alices_book).await?.is_empty() {
        return Err(oops("LEAK: bob read alice's spine"));
    }
    println!("bob's view of it is empty");

    println!("\n--- vocabulary ---");
    db.record_lookup(alice, alices_book, prose, "nice", "nice", "a nice distinction", "precise")
        .await?;
    db.record_lookup(alice, alices_book, prose, "Nice", "nice", "a nice point", "precise")
        .await?;
    db.record_lookup(alice, alices_book, prose, "want", "want", "they that want honesty", "lack")
        .await?;

    let vocab = db.vocabulary(alice, alices_book).await?;
    if vocab.len() != 2 || vocab[0].word != "nice" || vocab[0].count != 2 {
        return Err(oops(&format!("vocabulary is wrong: {vocab:?}")));
    }
    println!(
        "{} words, most frequent first: {:?}",
        vocab.len(),
        vocab.iter().map(|v| (&v.word, v.count)).collect::<Vec<_>>()
    );
    println!("the sentence came with it: {:?}", vocab[0].sentence);

    match db
        .record_lookup(bob, alices_book, prose, "sneak", "sneak", "s", "g")
        .await
    {
        Err(e) => println!("bob cannot record against alice's book: {e}"),
        Ok(_) => return Err(oops("LEAK: bob wrote into alice's vocabulary")),
    }

    println!("\n--- deleting a page returns its files ---");
    let files = db.delete_page(alice, page_one).await?;
    println!("delete_page handed back {files:?} to unlink");

    println!("\n--- settings ---");
    db.set_setting("check_db_probe", "first").await?;
    db.set_setting("check_db_probe", "second").await?;
    let value = db.get_setting("check_db_probe").await?;
    if value.as_deref() != Some("second") {
        return Err(oops("upsert did not overwrite"));
    }
    println!("upsert overwrites: {value:?}");

    println!("\n--- sign in ---");
    let email = format!("carol-{tag}@example.test");
    let carol = reading_core::auth::register(db, &email, "Carol", "a long enough one").await?;
    println!("registered {}", carol.get());

    match reading_core::auth::register(db, &email, "Carol", "short").await {
        Err(e) => println!("a short password is refused: {e}"),
        Ok(_) => return Err(oops("a short password was accepted")),
    }

    match reading_core::auth::sign_in(db, &email, "the wrong one", "test").await {
        Err(e) => println!("wrong password: {e}"),
        Ok(_) => return Err(oops("LEAK: signed in with the wrong password")),
    }
    match reading_core::auth::sign_in(db, "nobody@example.test", "a long enough one", "test").await {
        Err(e) => println!("unknown address:  {e}"),
        Ok(_) => return Err(oops("LEAK: signed in as a nonexistent user")),
    }

    let session = reading_core::auth::sign_in(db, &email, "a long enough one", "test").await?;
    println!("signed in, token is {} chars", session.token.len());

    let who = reading_core::auth::authenticate(db, &session.token).await?;
    if who.id != carol {
        return Err(oops("the token resolved to the wrong person"));
    }
    println!("token resolves to {}", who.email);

    match reading_core::auth::authenticate(db, "0".repeat(64).as_str()).await {
        Err(e) => println!("a made-up token is refused: {e}"),
        Ok(_) => return Err(oops("LEAK: a made-up token authenticated")),
    }

    // The stored form must not be the token, or a database read is a set of
    // working credentials.
    let stored = reading_core::sqlx::query(
        "SELECT count(*) AS n FROM sessions WHERE encode(token_hash, 'hex') = $1",
    )
    .bind(&session.token)
    .fetch_one(db.pool())
    .await
    .map_err(|e| oops(&format!("{e}")))?;
    if stored.get::<i64, _>("n") != 0 {
        return Err(oops("SERIOUS: the raw token is in the sessions table"));
    }
    println!("the raw token is not in the table, only its digest");

    reading_core::auth::sign_out(db, &session.token).await?;
    match reading_core::auth::authenticate(db, &session.token).await {
        Err(e) => println!("after signing out: {e}"),
        Ok(_) => return Err(oops("LEAK: the token still works after sign out")),
    }

    delete_user(db, carol).await?;

    println!("\n--- cleanup ---");
    db.delete_book(alice, alices_book).await?;
    db.delete_book(bob, bobs_book).await?;
    delete_user(db, alice).await?;
    delete_user(db, bob).await?;
    println!("removed the throwaway users and their books");

    println!("\nAll checks passed.");
    Ok(())
}

/// Not on `Db` on purpose: deleting a person and everything they have written
/// is not something the application should offer casually, and this is a test
/// fixture rather than a feature.
async fn delete_user(db: &Db, id: UserId) -> reading_core::Result<()> {
    reading_core::sqlx::query("DELETE FROM users WHERE id = $1")
        .bind(id.get())
        .execute(db.pool())
        .await
        .map(|_| ())
        .map_err(|e| oops(&format!("could not remove user: {e}")))
}

fn oops(message: &str) -> reading_core::AppError {
    reading_core::AppError::Other(anyhow::anyhow!(message.to_string()))
}

fn redact(url: &str) -> String {
    match (url.find("://"), url.rfind('@')) {
        (Some(scheme), Some(at)) if at > scheme => {
            format!("{}://***{}", &url[..scheme], &url[at..])
        }
        _ => url.to_string(),
    }
}
