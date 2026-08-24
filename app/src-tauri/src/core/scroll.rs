use std::{
    sync::mpsc::{Receiver, SyncSender},
    time::Duration,
};

use base64::{Engine, engine::general_purpose::STANDARD as BASE64};
use image::{RgbaImage, imageops, imageops::FilterType as ResizeFilter};
use serde::{Deserialize, Serialize};

use crate::core::image::png_bytes;

#[derive(Clone, Deserialize, Serialize)]
pub(crate) struct ScrollPreview {
    pub(crate) base64: String,
    pub(crate) width: u32,
    pub(crate) height: u32,
}

pub(crate) struct NativeScrollSession {
    pub(crate) width: u32,
    pub(crate) viewport_height: u32,
    pub(crate) total_height: u32,
    pub(crate) last_frame: RgbaImage,
    pub(crate) segments: Vec<RgbaImage>,
    pub(crate) preview_width: u32,
    pub(crate) preview_height: u32,
    pub(crate) preview_segments: Vec<RgbaImage>,
    pub(crate) last_observed_scrollbar: Option<VerticalScrollbar>,
    pub(crate) pending_scroll_delta: u32,
    pub(crate) last_scroll_delta: Option<u32>,
}

pub(crate) enum ScrollFrameMessage {
    Frame(RgbaImage),
    Finish,
    Cancel,
}

pub(crate) struct ScrollPipeline {
    pub(crate) frames: SyncSender<ScrollFrameMessage>,
    pub(crate) finished: Receiver<NativeScrollSession>,
}

pub(crate) const SCROLL_MIN_NEW_CONTENT: u32 = 4;
pub(crate) const SCROLL_MIN_OVERLAP: u32 = 12;
pub(crate) const SCROLL_MAX_MATCH_SCORE: f64 = 0.10;
pub(crate) const SCROLL_MAX_INFORMATIVE_SCORE: f64 = 0.16;
pub(crate) const SCROLL_DUPLICATE_SCORE: f64 = 0.018;
pub(crate) const SCROLL_DUPLICATE_INFORMATIVE_SCORE: f64 = 0.04;
pub(crate) const SCROLL_MIN_CONFIDENT_COLUMNS: usize = 3;
// Raw captures are lossless RGBA frames. Keep enough short-term headroom for
// Windows GDI capture and the native matcher to run at different speeds, while
// remaining memory-bounded for Retina/4K selections.
pub(crate) const SCROLL_FRAME_BUFFER_BYTES: usize = 256 * 1024 * 1024;
pub(crate) const SCROLL_IDLE_REFRESH_INTERVAL: Duration = Duration::from_millis(300);

#[derive(Clone, Copy)]
struct ScrollFeature {
    x: u32,
    y: u32,
    strength: f64,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct VerticalOverlap {
    pub(crate) overlap: u32,
    pub(crate) fixed_top: u32,
    pub(crate) fixed_bottom: u32,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct VerticalScrollbar {
    pub(crate) top: u32,
    pub(crate) length: u32,
    pub(crate) from_right: bool,
}

pub(crate) fn rgb_difference(a: image::Rgba<u8>, b: image::Rgba<u8>) -> f64 {
    (a[0].abs_diff(b[0]) as f64 + a[1].abs_diff(b[1]) as f64 + a[2].abs_diff(b[2]) as f64)
        / (255.0 * 3.0)
}

pub(crate) fn scroll_frame_fingerprint(image: &RgbaImage) -> u64 {
    // Screen capture frequently returns many byte-identical frames between
    // wheel events. Sending all of them through the expensive seam matcher can
    // fill the queue before real movement arrives, especially on Windows.
    // Sample a dense deterministic grid to cheaply suppress those duplicates;
    // a periodic forced refresh still captures lazy-loaded local changes that
    // happen between sample points.
    const SAMPLE_COLUMNS: u32 = 64;
    const SAMPLE_ROWS: u32 = 48;
    let width = image.width().max(1);
    let height = image.height().max(1);
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    let mut mix = |value: u64| {
        hash ^= value;
        hash = hash.wrapping_mul(0x100_0000_01b3);
    };
    mix(width as u64);
    mix(height as u64);
    for row in 0..SAMPLE_ROWS.min(height) {
        let y =
            (((row as u64 * 2 + 1) * height as u64) / (SAMPLE_ROWS.min(height) as u64 * 2)) as u32;
        for column in 0..SAMPLE_COLUMNS.min(width) {
            let x = (((column as u64 * 2 + 1) * width as u64)
                / (SAMPLE_COLUMNS.min(width) as u64 * 2)) as u32;
            let pixel = image.get_pixel(x.min(width - 1), y.min(height - 1)).0;
            mix(u32::from_le_bytes(pixel) as u64);
        }
    }
    hash
}

pub(crate) fn feature_strength(image: &RgbaImage, x: u32, y: u32, width: u32, height: u32) -> f64 {
    let mut strength = 0.0;
    let mut samples = 0_u32;
    for yy in (y..y + height - 1).step_by(3) {
        for xx in (x..x + width - 1).step_by(3) {
            strength += rgb_difference(*image.get_pixel(xx, yy), *image.get_pixel(xx + 1, yy));
            strength += rgb_difference(*image.get_pixel(xx, yy), *image.get_pixel(xx, yy + 1));
            samples += 2;
        }
    }
    strength / samples.max(1) as f64
}

pub(crate) fn patch_difference(
    previous: &RgbaImage,
    next: &RgbaImage,
    x: u32,
    previous_y: u32,
    next_y: u32,
    width: u32,
    height: u32,
) -> f64 {
    let mut scores = Vec::new();
    for yy in (0..height).step_by(3) {
        for xx in (0..width).step_by(3) {
            scores.push(rgb_difference(
                *previous.get_pixel(x + xx, previous_y + yy),
                *next.get_pixel(x + xx, next_y + yy),
            ));
        }
    }
    // Ignore the noisiest quarter of a patch so caret blinking and small
    // animations do not invalidate an otherwise exact text/edge match.
    scores.sort_by(f64::total_cmp);
    let retained = ((scores.len() as f64) * 0.75).ceil() as usize;
    scores.iter().take(retained).sum::<f64>() / retained.max(1) as f64
}

pub(crate) fn feature_scroll_deltas(
    previous: &RgbaImage,
    next: &RgbaImage,
    fixed_top: u32,
) -> Vec<u32> {
    const PATCH_WIDTH: u32 = 30;
    const PATCH_HEIGHT: u32 = 18;
    let width = previous.width().min(next.width());
    let height = previous.height().min(next.height());
    if width < PATCH_WIDTH + 8 || height < PATCH_HEIGHT + SCROLL_MIN_NEW_CONTENT + 8 {
        return Vec::new();
    }

    let mut features = Vec::new();
    for y in ((fixed_top + 4)..height.saturating_sub(PATCH_HEIGHT + 4)).step_by(30) {
        for x in (4..width.saturating_sub(PATCH_WIDTH + 4)).step_by(36) {
            let strength = feature_strength(previous, x, y, PATCH_WIDTH, PATCH_HEIGHT);
            if strength < 0.025 {
                continue;
            }
            let activity = patch_difference(previous, next, x, y, y, PATCH_WIDTH, PATCH_HEIGHT);
            if activity > 0.025 {
                features.push(ScrollFeature {
                    x,
                    y,
                    strength: strength * activity,
                });
            }
        }
    }
    features.sort_by(|a, b| b.strength.total_cmp(&a.strength));
    // Keep features across the full viewport. Large scroll steps can only be
    // measured by features that started near the bottom; selecting only the
    // globally strongest few biases the vote toward small, incorrect shifts.
    features.truncate(72);

    let mut votes = Vec::new();
    for feature in features {
        let max_delta = feature
            .y
            .saturating_sub(fixed_top)
            .min(height - SCROLL_MIN_OVERLAP);
        if max_delta < SCROLL_MIN_NEW_CONTENT {
            continue;
        }
        let mut best_delta = 0;
        let mut best_score = f64::INFINITY;
        let mut scored = Vec::new();
        for delta in SCROLL_MIN_NEW_CONTENT..=max_delta {
            let score = patch_difference(
                previous,
                next,
                feature.x,
                feature.y,
                feature.y - delta,
                PATCH_WIDTH,
                PATCH_HEIGHT,
            );
            scored.push((delta, score));
            if score < best_score {
                best_score = score;
                best_delta = delta;
            }
        }
        let second_score = scored
            .iter()
            .filter(|(delta, _)| delta.abs_diff(best_delta) > 2)
            .map(|(_, score)| *score)
            .fold(f64::INFINITY, f64::min);
        if best_score <= 0.065 && (best_score <= 0.008 || second_score - best_score >= 0.008) {
            votes.push(best_delta);
        }
    }

    if votes.len() < 2 {
        return Vec::new();
    }
    let mut candidates = Vec::new();
    for &candidate in &votes {
        let mut cluster = votes
            .iter()
            .copied()
            .filter(|vote| vote.abs_diff(candidate) <= 2)
            .collect::<Vec<_>>();
        if cluster.len() >= 2 {
            cluster.sort_unstable();
            let median = cluster[cluster.len() / 2];
            if !candidates
                .iter()
                .any(|existing: &u32| existing.abs_diff(median) <= 2)
            {
                candidates.push(median);
            }
        }
    }
    candidates
}

pub(crate) fn scroll_active_columns(previous: &RgbaImage, next: &RgbaImage) -> Vec<u32> {
    let width = previous.width().min(next.width());
    let left = width * 5 / 100;
    let right = (width * 95 / 100).max(left + 1);
    let step = ((right - left) / 64).max(1);
    let height = previous.height().min(next.height());
    let top = height * 10 / 100;
    let bottom = (height * 90 / 100).max(top + 1);
    let y_step = ((bottom - top) / 18).max(1);
    let mut active = Vec::new();
    for x in (left..right).step_by(step as usize) {
        let mut differences = Vec::new();
        for y in (top..bottom).step_by(y_step as usize) {
            let a = previous.get_pixel(x, y).0;
            let b = next.get_pixel(x, y).0;
            differences.push(
                (a[0].abs_diff(b[0]) as f64
                    + a[1].abs_diff(b[1]) as f64
                    + a[2].abs_diff(b[2]) as f64)
                    / (255.0 * 3.0),
            );
        }
        differences.sort_by(|a, b| b.total_cmp(a));
        let retained = 3_usize.max(differences.len() / 3).min(differences.len());
        let activity = differences.iter().take(retained).sum::<f64>() / retained.max(1) as f64;
        if activity > 0.018 {
            active.push(x);
        }
    }
    active
}

pub(crate) fn scroll_sample_columns(width: u32) -> Vec<u32> {
    if width == 0 {
        return Vec::new();
    }
    let left = width * 5 / 100;
    let right = (width * 95 / 100).max(left + 1);
    let samples = 72_u32.min((right - left).max(1));
    (0..samples)
        .map(|index| {
            left + (((index as f64 + 0.5) * (right - left) as f64) / samples as f64) as u32
        })
        .map(|x| x.min(width - 1))
        .collect()
}

pub(crate) fn detect_vertical_scrollbar(image: &RgbaImage) -> Option<VerticalScrollbar> {
    let width = image.width();
    let height = image.height();
    if width < 40 || height < 80 {
        return None;
    }
    let band = 14_u32.min(width / 4);
    let reference_gap = 8_u32;
    let mut best: Option<(VerticalScrollbar, u32)> = None;

    for from_right in [true, false] {
        let reference_x = if from_right {
            width.saturating_sub(band + reference_gap + 1)
        } else {
            (band + reference_gap).min(width - 1)
        };
        let mut active_rows = Vec::with_capacity(height as usize);
        for y in 0..height {
            let reference = *image.get_pixel(reference_x, y);
            let changed = (0..band)
                .filter(|offset| {
                    let x = if from_right {
                        width - 1 - offset
                    } else {
                        *offset
                    };
                    rgb_difference(*image.get_pixel(x, y), reference) >= 0.045
                })
                .count() as u32;
            // Scrollbar thumbs occupy a narrow edge strip. Reject full-width
            // edge colour changes from page sections, dialogs and borders.
            active_rows.push(changed >= 2 && changed <= band.saturating_sub(2));
        }

        let mut start = 0_u32;
        while start < height {
            while start < height && !active_rows[start as usize] {
                start += 1;
            }
            if start >= height {
                break;
            }
            let mut end = start;
            let mut last_active = start;
            let mut gap = 0_u32;
            while end < height {
                if active_rows[end as usize] {
                    last_active = end;
                    gap = 0;
                } else {
                    gap += 1;
                    if gap > 2 {
                        break;
                    }
                }
                end += 1;
            }
            let length = last_active + 1 - start;
            if length >= 12 && length <= height * 80 / 100 {
                let candidate = VerticalScrollbar {
                    top: start,
                    length,
                    from_right,
                };
                let score = length;
                if best
                    .map(|(_, best_score)| score > best_score)
                    .unwrap_or(true)
                {
                    best = Some((candidate, score));
                }
            }
            start = end.max(start + 1);
        }
    }
    best.map(|(scrollbar, _)| scrollbar)
}

pub(crate) fn scrollbar_scroll_delta(
    previous: VerticalScrollbar,
    next: VerticalScrollbar,
    viewport_height: u32,
) -> Option<u32> {
    if previous.from_right != next.from_right || next.top <= previous.top {
        return None;
    }
    let average_length = (previous.length + next.length) / 2;
    if average_length < 8 || previous.length.abs_diff(next.length) > average_length / 3 {
        return None;
    }
    let delta = next
        .top
        .saturating_sub(previous.top)
        .saturating_mul(viewport_height)
        / average_length.max(1);
    (delta >= SCROLL_MIN_NEW_CONTENT).then_some(delta)
}

pub(crate) fn same_row_score(
    previous: &RgbaImage,
    next: &RgbaImage,
    y: u32,
    columns: &[u32],
) -> f64 {
    let mut differences = Vec::with_capacity(columns.len());
    for &x in columns {
        let a = previous.get_pixel(x, y).0;
        let b = next.get_pixel(x, y).0;
        differences.push(
            (a[0].abs_diff(b[0]) as f64 + a[1].abs_diff(b[1]) as f64 + a[2].abs_diff(b[2]) as f64)
                / (255.0 * 3.0),
        );
    }
    differences.sort_by(f64::total_cmp);
    let retained = 3_usize
        .max(((differences.len() as f64) * 0.75).ceil() as usize)
        .min(differences.len());
    differences.iter().take(retained).sum::<f64>() / retained.max(1) as f64
}

pub(crate) fn stable_edge_height(
    previous: &RgbaImage,
    next: &RgbaImage,
    from_top: bool,
    columns: &[u32],
) -> u32 {
    let height = previous.height().min(next.height());
    let limit = height * 45 / 100;
    let mut last_stable = 0;
    let mut misses = 0;
    for offset in 0..limit {
        let y = if from_top {
            offset
        } else {
            height - 1 - offset
        };
        if same_row_score(previous, next, y, columns) <= 0.025 {
            last_stable = offset + 1;
            misses = 0;
        } else {
            misses += 1;
            if misses >= 4 {
                break;
            }
        }
    }
    if last_stable >= 8 { last_stable } else { 0 }
}

pub(crate) fn native_overlap_score(
    previous: &RgbaImage,
    next: &RgbaImage,
    overlap: u32,
    fixed_top: u32,
    fixed_bottom: u32,
    columns: &[u32],
) -> f64 {
    // Ignore stable top/bottom chrome (sticky navigation, cookie bars, media
    // controls) and small safety margins. Only actual scrolling content should
    // decide the vertical offset.
    let top_guard = fixed_top
        .max(previous.height() * 12 / 100)
        .min(overlap.saturating_sub(12));
    let after_top = overlap.saturating_sub(top_guard);
    let bottom_guard = fixed_bottom
        .max(previous.height() * 6 / 100)
        .min(after_top.saturating_sub(8))
        .min(after_top / 3);
    let usable = overlap.saturating_sub(top_guard + bottom_guard).max(1);
    let rows = 24_u32.min(8_u32.max(usable / 10));
    let mut row_scores = Vec::with_capacity(rows as usize);

    for row in 0..rows {
        let offset = top_guard
            + (((row as f64 + 0.5) * usable as f64) / rows as f64)
                .floor()
                .min((usable - 1) as f64) as u32;
        let previous_y = previous.height() - overlap + offset;
        let next_y = offset;
        let mut column_scores = Vec::with_capacity(columns.len());
        for &x in columns {
            let a = previous.get_pixel(x, previous_y).0;
            let b = next.get_pixel(x, next_y).0;
            column_scores.push(
                (a[0].abs_diff(b[0]) as f64
                    + a[1].abs_diff(b[1]) as f64
                    + a[2].abs_diff(b[2]) as f64)
                    / (255.0 * 3.0),
            );
        }
        column_scores.sort_by(f64::total_cmp);
        let retained_columns = 3_usize
            .max(((column_scores.len() as f64) * 0.75).ceil() as usize)
            .min(column_scores.len());
        row_scores.push(
            column_scores.iter().take(retained_columns).sum::<f64>()
                / retained_columns.max(1) as f64,
        );
    }

    row_scores.sort_by(f64::total_cmp);
    let retained = 5_usize.max(((row_scores.len() as f64) * 0.7).ceil() as usize);
    row_scores.iter().take(retained).sum::<f64>() / retained as f64
}

pub(crate) fn pixel_edge_activity(image: &RgbaImage, x: u32, y: u32) -> f64 {
    let left = x.saturating_sub(1);
    let right = (x + 1).min(image.width() - 1);
    let top = y.saturating_sub(1);
    let bottom = (y + 1).min(image.height() - 1);
    let pixel = *image.get_pixel(x, y);
    rgb_difference(pixel, *image.get_pixel(left, y))
        .max(rgb_difference(pixel, *image.get_pixel(right, y)))
        .max(rgb_difference(pixel, *image.get_pixel(x, top)))
        .max(rgb_difference(pixel, *image.get_pixel(x, bottom)))
}

pub(crate) fn informative_overlap_score(
    previous: &RgbaImage,
    next: &RgbaImage,
    overlap: u32,
    fixed_top: u32,
    fixed_bottom: u32,
    columns: &[u32],
) -> Option<f64> {
    // Product/review pages contain many repeated white rows, separators and
    // rating bars. A broad average can align the wrong review perfectly while
    // ignoring the few rows that contain its unique text. Validate candidates
    // again using only actual edges (glyphs, icons and image detail).
    let top_guard = fixed_top
        .max(previous.height() * 12 / 100)
        .min(overlap.saturating_sub(12));
    let after_top = overlap.saturating_sub(top_guard);
    let bottom_guard = fixed_bottom
        .max(previous.height() * 6 / 100)
        .min(after_top.saturating_sub(8))
        .min(after_top / 3);
    let usable = overlap.saturating_sub(top_guard + bottom_guard);
    if usable == 0 || columns.is_empty() {
        return None;
    }

    let rows = 96_u32.min(usable).max(1);
    let mut differences = Vec::with_capacity(rows as usize * columns.len());
    for row in 0..rows {
        let offset = top_guard
            + (((row as f64 + 0.5) * usable as f64) / rows as f64)
                .floor()
                .min((usable - 1) as f64) as u32;
        let previous_y = previous.height() - overlap + offset;
        let next_y = offset;
        for &x in columns {
            let x = x.min(previous.width() - 1).min(next.width() - 1);
            let activity = pixel_edge_activity(previous, x, previous_y)
                .max(pixel_edge_activity(next, x, next_y));
            if activity >= 0.035 {
                differences.push(rgb_difference(
                    *previous.get_pixel(x, previous_y),
                    *next.get_pixel(x, next_y),
                ));
            }
        }
    }
    if differences.len() < columns.len().max(24) {
        return None;
    }
    differences.sort_by(f64::total_cmp);
    // Ignore only a small noisy tail for GIFs/carets. Unlike the broad score,
    // keep nearly all text edges so repeated card templates cannot win by
    // matching their blank background and common rating bar.
    let retained = ((differences.len() as f64) * 0.92).ceil() as usize;
    Some(differences.iter().take(retained).sum::<f64>() / retained.max(1) as f64)
}

pub(crate) fn robust_overlap_score(
    previous: &RgbaImage,
    next: &RgbaImage,
    overlap: u32,
    fixed_top: u32,
    fixed_bottom: u32,
    columns: &[u32],
    broad_score: f64,
) -> f64 {
    informative_overlap_score(previous, next, overlap, fixed_top, fixed_bottom, columns)
        .map(|informative| broad_score * 0.35 + informative * 0.65)
        .unwrap_or(broad_score)
}

pub(crate) fn overlap_match_rank(
    score: f64,
    delta: u32,
    viewport: u32,
    expected: Option<u32>,
) -> f64 {
    let distance = expected
        .map(|expected| delta.abs_diff(expected))
        // The first changed frame is sampled immediately after the baseline;
        // prefer the nearer valid seam when repeated list rows are otherwise
        // similarly convincing.
        .unwrap_or(delta);
    let weight = if expected.is_some() { 0.08 } else { 0.012 };
    score + distance as f64 / viewport.max(1) as f64 * weight
}

#[cfg(test)]
pub(crate) fn native_vertical_overlap(
    previous: &RgbaImage,
    next: &RgbaImage,
) -> Option<VerticalOverlap> {
    native_vertical_overlap_with_hint(previous, next, None)
}

pub(crate) fn native_vertical_overlap_with_hint(
    previous: &RgbaImage,
    next: &RgbaImage,
    expected_delta: Option<u32>,
) -> Option<VerticalOverlap> {
    if previous.dimensions() != next.dimensions() {
        return None;
    }
    let viewport = previous.height();
    let max_overlap = viewport.checked_sub(SCROLL_MIN_NEW_CONTENT)?;
    if max_overlap < SCROLL_MIN_OVERLAP {
        return None;
    }

    let active_columns = scroll_active_columns(previous, next);
    let sample_columns;
    let columns = if active_columns.len() >= SCROLL_MIN_CONFIDENT_COLUMNS {
        active_columns.as_slice()
    } else {
        // Low-texture pages can scroll with only a few changing columns. Do
        // not call those frames duplicates by default; use broad sampling so
        // blank margins and sparse layouts still have a chance to align.
        sample_columns = scroll_sample_columns(previous.width().min(next.width()));
        sample_columns.as_slice()
    };

    let fixed_top = stable_edge_height(previous, next, true, &columns);
    let fixed_bottom = stable_edge_height(previous, next, false, &columns);

    let full_score =
        native_overlap_score(previous, next, viewport, fixed_top, fixed_bottom, &columns);
    let full_robust_score = robust_overlap_score(
        previous,
        next,
        viewport,
        fixed_top,
        fixed_bottom,
        &columns,
        full_score,
    );
    if full_score <= SCROLL_DUPLICATE_SCORE
        && full_robust_score <= SCROLL_DUPLICATE_INFORMATIVE_SCORE
    {
        return Some(VerticalOverlap {
            overlap: viewport,
            fixed_top,
            fixed_bottom,
        });
    }

    // Feature patches propose possible shifts; validate every consensus with
    // the wider scrolling region so repeated text/patterns cannot select a
    // locally convincing but globally incorrect seam.
    let mut feature_match: Option<(u32, f64, f64, f64)> = None;
    for delta in feature_scroll_deltas(previous, next, fixed_top) {
        let overlap = viewport - delta;
        let broad_score =
            native_overlap_score(previous, next, overlap, fixed_top, fixed_bottom, &columns);
        let robust_score = robust_overlap_score(
            previous,
            next,
            overlap,
            fixed_top,
            fixed_bottom,
            &columns,
            broad_score,
        );
        let rank = overlap_match_rank(robust_score, delta, viewport, expected_delta);
        if feature_match
            .map(|(_, _, _, best_rank)| rank < best_rank)
            .unwrap_or(true)
        {
            feature_match = Some((overlap, broad_score, robust_score, rank));
        }
    }
    if let Some((overlap, broad_score, robust_score, _)) = feature_match {
        if broad_score <= SCROLL_MAX_MATCH_SCORE && robust_score <= SCROLL_MAX_INFORMATIVE_SCORE {
            return Some(VerticalOverlap {
                overlap,
                fixed_top,
                fixed_bottom,
            });
        }
    }

    // When substantial unchanged bands remain at both edges but no textured
    // feature can prove a translation, the page changed in place (most often a
    // lazy-loaded image, web font or skeleton). Requiring both edges avoids
    // dropping real scroll progress on pages with a tall sticky header or a
    // large fixed footer.
    if fixed_top >= 8 && fixed_bottom >= 8 && fixed_top + fixed_bottom >= viewport * 35 / 100 {
        return Some(VerticalOverlap {
            overlap: viewport,
            fixed_top,
            fixed_bottom,
        });
    }

    let mut candidates = Vec::new();
    for overlap in (SCROLL_MIN_OVERLAP..=max_overlap).rev() {
        let score =
            native_overlap_score(previous, next, overlap, fixed_top, fixed_bottom, &columns);
        if score <= SCROLL_MAX_MATCH_SCORE {
            candidates.push((overlap, score));
        }
    }
    if candidates.is_empty() {
        return None;
    }

    // Broad scoring intentionally tolerates animation, but that also produces
    // several deceptively good offsets on repeated review cards. Validate both
    // the globally best broad candidates and the candidates nearest the recent
    // physical movement, then choose by text/detail score plus continuity.
    candidates.sort_by(|a, b| a.1.total_cmp(&b.1));
    let mut shortlist = candidates.iter().take(32).copied().collect::<Vec<_>>();
    candidates.sort_by_key(|(overlap, _)| {
        let delta = viewport - *overlap;
        expected_delta
            .map(|expected| delta.abs_diff(expected))
            .unwrap_or(delta)
    });
    for candidate in candidates.iter().take(16).copied() {
        if !shortlist.iter().any(|(overlap, _)| *overlap == candidate.0) {
            shortlist.push(candidate);
        }
    }

    let mut best: Option<(u32, f64)> = None;
    for (overlap, broad_score) in shortlist {
        let robust_score = robust_overlap_score(
            previous,
            next,
            overlap,
            fixed_top,
            fixed_bottom,
            &columns,
            broad_score,
        );
        if robust_score > SCROLL_MAX_INFORMATIVE_SCORE {
            continue;
        }
        let delta = viewport - overlap;
        let rank = overlap_match_rank(robust_score, delta, viewport, expected_delta);
        if best.map(|(_, best_rank)| rank < best_rank).unwrap_or(true) {
            best = Some((overlap, rank));
        }
    }
    let (best_overlap, _) = best?;
    Some(VerticalOverlap {
        overlap: best_overlap,
        fixed_top,
        fixed_bottom,
    })
}

pub(crate) fn native_overlap_near_scroll_delta(
    previous: &RgbaImage,
    next: &RgbaImage,
    predicted_delta: u32,
) -> Option<VerticalOverlap> {
    if previous.dimensions() != next.dimensions() {
        return None;
    }
    let viewport = previous.height();
    let max_delta = viewport.saturating_sub(SCROLL_MIN_NEW_CONTENT);
    if predicted_delta < SCROLL_MIN_NEW_CONTENT || max_delta < SCROLL_MIN_OVERLAP {
        return None;
    }

    let active_columns = scroll_active_columns(previous, next);
    let sample_columns;
    let columns = if active_columns.len() >= SCROLL_MIN_CONFIDENT_COLUMNS {
        active_columns.as_slice()
    } else {
        sample_columns = scroll_sample_columns(previous.width().min(next.width()));
        sample_columns.as_slice()
    };
    let fixed_top = stable_edge_height(previous, next, true, columns);
    let fixed_bottom = stable_edge_height(previous, next, false, columns);
    let radius = 24_u32.max(predicted_delta / 8);
    let start = predicted_delta
        .saturating_sub(radius)
        .max(SCROLL_MIN_NEW_CONTENT);
    let end = predicted_delta.saturating_add(radius).min(max_delta);
    if start > end {
        return None;
    }

    let mut best: Option<(u32, f64, f64)> = None;
    for delta in start..=end {
        let overlap = viewport - delta;
        let broad_score =
            native_overlap_score(previous, next, overlap, fixed_top, fixed_bottom, &columns);
        let robust_score = robust_overlap_score(
            previous,
            next,
            overlap,
            fixed_top,
            fixed_bottom,
            &columns,
            broad_score,
        );
        let rank = overlap_match_rank(robust_score, delta, viewport, Some(predicted_delta));
        if best
            .map(|(_, _, best_rank)| rank < best_rank)
            .unwrap_or(true)
        {
            best = Some((delta, broad_score, rank));
        }
    }
    let (delta, broad_score, _) = best?;
    let overlap = viewport - delta;
    let robust_score = robust_overlap_score(
        previous,
        next,
        overlap,
        fixed_top,
        fixed_bottom,
        &columns,
        broad_score,
    );
    (broad_score <= SCROLL_MAX_MATCH_SCORE && robust_score <= SCROLL_MAX_INFORMATIVE_SCORE)
        .then_some(VerticalOverlap {
            overlap: viewport - delta,
            fixed_top,
            fixed_bottom,
        })
}

pub(crate) fn stationary_refresh_overlap(
    previous: &RgbaImage,
    next: &RgbaImage,
) -> Option<VerticalOverlap> {
    if previous.dimensions() != next.dimensions() {
        return None;
    }
    let viewport = previous.height();
    let active_columns = scroll_active_columns(previous, next);
    let sample_columns;
    let columns = if active_columns.len() >= SCROLL_MIN_CONFIDENT_COLUMNS {
        active_columns.as_slice()
    } else {
        sample_columns = scroll_sample_columns(previous.width().min(next.width()));
        sample_columns.as_slice()
    };
    let fixed_top = stable_edge_height(previous, next, true, columns);
    let fixed_bottom = stable_edge_height(previous, next, false, columns);
    // A single fixed edge is not enough evidence that the page stayed still:
    // product pages commonly have a tall sticky header or footer while the
    // content underneath continues to move. Only refresh the committed frame
    // when either both edges are stable, or most sampled rows in the actually
    // changing columns remain aligned at the same coordinates. The latter
    // covers lazy-loaded images without turning a sticky header into a false
    // "stationary" result.
    let stable_same_position = if active_columns.len() >= SCROLL_MIN_CONFIDENT_COLUMNS {
        let top = viewport * 5 / 100;
        let bottom = (viewport * 95 / 100).max(top + 1);
        let rows = 36_u32.min((bottom - top).max(1));
        let stable_rows = (0..rows)
            .filter(|row| {
                let y = top + (((*row as f64 + 0.5) * (bottom - top) as f64) / rows as f64) as u32;
                same_row_score(previous, next, y.min(viewport - 1), columns) <= 0.025
            })
            .count();
        stable_rows * 100 >= rows as usize * 55
    } else {
        false
    };
    if (fixed_top >= 8 && fixed_bottom >= 8 && fixed_top + fixed_bottom >= viewport * 35 / 100)
        || stable_same_position
    {
        return Some(VerticalOverlap {
            overlap: viewport,
            fixed_top,
            fixed_bottom,
        });
    }
    None
}

impl NativeScrollSession {
    pub(crate) fn new(initial: RgbaImage) -> Self {
        // Keep the live preview substantially larger than the control window.
        // The final image always uses the original frames; this only prevents
        // small review/comment text from becoming unreadable in the preview.
        let preview_width = initial.width().min(720).max(1);
        let preview_height = ((initial.height() as f64 * preview_width as f64)
            / initial.width().max(1) as f64)
            .round()
            .max(1.0) as u32;
        let preview = imageops::resize(
            &initial,
            preview_width,
            preview_height,
            ResizeFilter::Triangle,
        );
        let last_observed_scrollbar = detect_vertical_scrollbar(&initial);
        Self {
            width: initial.width(),
            viewport_height: initial.height(),
            total_height: initial.height(),
            last_frame: initial.clone(),
            segments: vec![initial],
            preview_width,
            preview_height,
            preview_segments: vec![preview],
            last_observed_scrollbar,
            pending_scroll_delta: 0,
            last_scroll_delta: None,
        }
    }

    fn refresh_overlap(&mut self, next: &RgbaImage, matched: VerticalOverlap) {
        // A later frame can contain images/fonts that finished loading after
        // their placeholder was first appended. Refresh the already-stitched
        // overlap from the newest frame instead of discarding it as a
        // duplicate. Fixed chrome is excluded so sticky headers are still kept
        // only once at the top of the long screenshot.
        let overlap = matched.overlap.min(self.viewport_height);
        let source_start = matched.fixed_top.min(overlap);
        let source_end = overlap.min(self.viewport_height.saturating_sub(matched.fixed_bottom));
        if source_end <= source_start || self.total_height < overlap {
            return;
        }
        let copy_x = 0;
        let copy_width = self.width;

        let destination_start = self.total_height - overlap + source_start;
        let destination_end = self.total_height - overlap + source_end;
        let mut segment_start = 0_u32;
        for segment in &mut self.segments {
            let segment_end = segment_start + segment.height();
            let copy_start = segment_start.max(destination_start);
            let copy_end = segment_end.min(destination_end);
            if copy_end > copy_start {
                let source_y = source_start + copy_start - destination_start;
                let source =
                    imageops::crop_imm(next, copy_x, source_y, copy_width, copy_end - copy_start)
                        .to_image();
                imageops::replace(
                    segment,
                    &source,
                    copy_x as i64,
                    (copy_start - segment_start) as i64,
                );
            }
            segment_start = segment_end;
            if segment_start >= destination_end {
                break;
            }
        }
    }

    pub(crate) fn append(&mut self, next: RgbaImage) -> bool {
        let next_scrollbar = detect_vertical_scrollbar(&next);
        if let (Some(previous), Some(current)) = (self.last_observed_scrollbar, next_scrollbar) {
            if let Some(delta) = scrollbar_scroll_delta(previous, current, self.viewport_height) {
                self.pending_scroll_delta = self.pending_scroll_delta.saturating_add(delta);
            }
        }
        if next_scrollbar.is_some() {
            self.last_observed_scrollbar = next_scrollbar;
        }

        let matched =
            native_vertical_overlap_with_hint(&self.last_frame, &next, self.last_scroll_delta);
        // When an on-screen scrollbar is visible, its thumb displacement is an
        // independent measurement of the actual scroll position. It is the
        // source of truth for the seam: repeated review rows, animated GIFs
        // and large flat colour blocks can produce a plausible but incorrect
        // pixel overlap, while the scrollbar still reports the exact movement.
        if self.pending_scroll_delta >= SCROLL_MIN_NEW_CONTENT {
            let scrollbar_delta = self.pending_scroll_delta;
            // Scrollbar thumb positions are quantized on long documents. Use
            // them to constrain the search, then snap to the best actual pixel
            // seam within a small neighbourhood so text is never split and
            // rendered twice at an inaccurate boundary.
            let refined =
                native_overlap_near_scroll_delta(&self.last_frame, &next, scrollbar_delta);
            let overlap = refined.map(|matched| matched.overlap).unwrap_or_else(|| {
                self.viewport_height
                    .saturating_sub(scrollbar_delta.min(self.viewport_height))
            });
            self.last_frame = next.clone();
            self.pending_scroll_delta = 0;
            // There is no pixel seam to refresh when the page is animated;
            // append exactly the rows implied by the physical movement.
            return self.append_tail(next, overlap);
        }

        let Some(matched) = matched else {
            // Animated GIFs and video-like product panels may have no stable
            // pixels to align. If the scrollbar moved only a small amount and
            // the visual matcher also failed, keep the committed anchor so a
            // later frame can still recover the complete distance.
            if let Some(refresh) = stationary_refresh_overlap(&self.last_frame, &next) {
                self.refresh_overlap(&next, refresh);
                // The frame is known to represent the same scroll position, so
                // it is safe to make its newly loaded pixels the committed
                // stitching anchor.
                self.last_frame = next;
            }
            // If movement cannot be proven, keep the last frame that actually
            // corresponds to the tail of `segments`. Advancing the anchor here
            // would discard the unmeasured distance; a later successful match
            // would then append only the newest part and leave a permanent gap.
            return false;
        };
        self.refresh_overlap(&next, matched);
        self.last_frame = next.clone();
        self.pending_scroll_delta = 0;
        self.append_tail(next, matched.overlap)
    }

    fn append_tail(&mut self, next: RgbaImage, overlap: u32) -> bool {
        let new_height = self.viewport_height.saturating_sub(overlap);
        if new_height < SCROLL_MIN_NEW_CONTENT {
            return false;
        }
        self.last_scroll_delta = Some(new_height);
        // Keep every horizontal pixel that was visible in the viewport. Pages
        // such as YouTube can lazy-load or animate independent columns; trying
        // to infer one "moving" column blanks genuine left/right content.
        let segment = imageops::crop_imm(&next, 0, overlap, self.width, new_height).to_image();
        let preview_height = ((new_height as f64 * self.preview_width as f64)
            / self.width.max(1) as f64)
            .round()
            .max(1.0) as u32;
        self.preview_segments.push(imageops::resize(
            &segment,
            self.preview_width,
            preview_height,
            ResizeFilter::Triangle,
        ));
        self.preview_height += preview_height;
        self.segments.push(segment);
        self.total_height += new_height;
        true
    }

    pub(crate) fn render(&self) -> RgbaImage {
        let mut output = RgbaImage::new(self.width, self.total_height);
        let mut y = 0_i64;
        for segment in &self.segments {
            imageops::replace(&mut output, segment, 0, y);
            y += segment.height() as i64;
        }
        output
    }

    pub(crate) fn preview_snapshot(&self) -> (RgbaImage, u32, u32) {
        let mut output = RgbaImage::new(self.preview_width, self.preview_height);
        let mut y = 0_i64;
        for segment in &self.preview_segments {
            imageops::replace(&mut output, segment, 0, y);
            y += segment.height() as i64;
        }
        (output, self.preview_width, self.preview_height)
    }
}

pub(crate) fn encode_scroll_preview(
    snapshot: (RgbaImage, u32, u32),
) -> Result<ScrollPreview, String> {
    Ok(ScrollPreview {
        base64: BASE64.encode(png_bytes(&snapshot.0)?),
        width: snapshot.1,
        height: snapshot.2,
    })
}
