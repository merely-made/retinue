//! The sheet.
//!
//! Plain CSS over Cambium's ordinary DOM. The Network face also installs
//! Cambium's shared graph-canvas sheet; its paint and native hit targets remain
//! one component rather than an app-local imitation.
//!
//! One rule is load-bearing rather than cosmetic. Genet's UA default makes
//! `button` and `input` `inline-block`, and inline-level boxes share their
//! line's fragment rather than getting one each — so an inline control has no
//! rect of its own, cannot be resolved by a `genet-probe` selector, and cannot
//! be given accurate bounds in the accessibility tree. Every control here is
//! block-level for that reason. See the G1 receipt in genet's docs.

pub const SHEET: &str = "
.shell {
    display: flex;
    background: #12151b;
    color: #e8e6e1;
    font-size: 15px;
    min-height: 100%;
}

/* The trail. Read-only: the flow owns page transitions. */
.trail {
    width: 210px;
    padding: 24px 16px;
    background: #171b23;
    margin: 0;
}
.trail-step {
    display: block;
    padding: 7px 10px;
    margin-bottom: 2px;
    color: #7e8896;
}
.trail-step.done { color: #8fbf9a; }
.trail-step.here { color: #e8e6e1; background: #222a36; }

.page { display: block; padding: 28px 32px; }
.page-head { display: block; margin-bottom: 20px; }
.page-title { display: block; font-size: 24px; margin: 0 0 6px 0; }
.page-subtitle { display: block; color: #98a2b1; }

/* Field rows: the review page's unit. */
.group {
    display: block;
    padding: 12px 14px;
    margin-bottom: 14px;
    background: #171b23;
}
.field { display: block; margin-bottom: 8px; }
.field-label {
    display: block;
    color: #98a2b1;
    font-size: 13px;
}
.field-value { display: block; }

.rows { display: block; margin-bottom: 16px; }
.row {
    display: block;
    width: 620px;
    padding: 10px 12px;
    margin-bottom: 4px;
    background: #1c222c;
    color: #e8e6e1;
    border: 1px solid #1c222c;
    text-align: left;
}
.row:hover { background: #232b37; }
.row.selected { border: 1px solid #6f9fd8; background: #232b37; }
.row:focus { border: 1px solid #a8c8ee; }

.revision-row { display: block; margin-bottom: 16px; }
/* The label wraps the field, so it must be block-level too — an inline label
   puts its <input> back in a shared line fragment, where it has no rect of its
   own and a reader's cursor has nothing to land on. */
.revision-label { display: block; }
.revision-wrap { display: block; margin: 6px 0; }
/* The tag, not a class: `text_field` renders a bare `<input>` and gives it no
   class of its own, so a class selector here would silently style nothing —
   which is exactly how the field came out invisible the first time. */
input {
    display: block;
    width: 240px;
    padding: 8px 10px;
    background: #1c222c;
    color: #e8e6e1;
    border: 1px solid #2b3441;
}
input:focus { border: 1px solid #a8c8ee; }
.hint { display: block; color: #7e8896; font-size: 13px; margin-top: 4px; width: 620px; }

.actions { display: block; margin-top: 18px; }
.primary, .secondary {
    display: block;
    width: 260px;
    padding: 11px 14px;
    margin-bottom: 8px;
    background: #2f5b8c;
    color: #f2f5f9;
    border: 1px solid #2f5b8c;
}
.primary:hover { background: #3a6da6; }
.primary:focus { border: 1px solid #a8c8ee; }
.secondary { background: #262e3a; border: 1px solid #262e3a; }
.secondary:hover { background: #2f3947; }
.secondary:focus { border: 1px solid #a8c8ee; }

/* A refusal is a visible state, never a disabled control. */
.refusal {
    display: block;
    width: 620px;
    padding: 12px 14px;
    margin-top: 18px;
    background: #2a1e1e;
    border: 1px solid #7d4040;
}
.refusal-title { display: block; color: #efb2b2; margin-bottom: 6px; }
.refusal-list { display: block; margin: 0; }
.refusal-line { display: block; margin-bottom: 4px; }

.empty { display: block; color: #98a2b1; width: 620px; margin-bottom: 14px; }
.instructions {
    display: block;
    width: 620px;
    padding: 12px 14px;
    margin-bottom: 14px;
    background: #1e2530;
    border: 1px solid #33405a;
}

.bar {
    display: block;
    width: 620px;
    height: 12px;
    background: #1c222c;
    margin-bottom: 16px;
}
.bar-fill { display: block; height: 12px; background: #6f9fd8; }

.notes { display: block; width: 620px; margin: 0; }
.note { display: block; padding: 3px 0; color: #c8ced8; }

.app-shell { display: block; min-height: 100%; background: #12151b; color: #e8e6e1; }
.section-tabs { display: flex; padding: 10px 16px; background: #0e1116; }
.section-tab {
    display: block;
    width: 130px;
    padding: 9px 12px;
    margin-right: 6px;
    color: #c8ced8;
    background: #1c222c;
    border: 1px solid #2b3441;
}
.section-tab.selected { color: #f2f5f9; background: #2f5b8c; }
.unavailable-page { display: block; padding: 28px 32px; }
.unavailable-gate {
    display: block;
    width: 620px;
    padding: 14px 16px;
    color: #c8ced8;
    background: #171b23;
    border: 1px solid #33405a;
}
.messages-page { display: block; padding: 28px 32px; }
.message-compose, .message-contact {
    display: block;
    width: 620px;
    padding: 14px 16px;
    margin-bottom: 18px;
    background: #171b23;
}
.message-rows { display: block; width: 760px; }
.message-row {
    display: block;
    width: 760px;
    padding: 10px 12px;
    margin-bottom: 4px;
    color: #e8e6e1;
    background: #1c222c;
    border: 1px solid #2b3441;
    text-align: left;
}
.message-row.selected { border-color: #a8c8ee; background: #263548; }
.message-notice { display: block; margin-top: 8px; color: #c8ced8; }
.network-page { display: block; padding: 28px 32px; }
.network-controls { display: flex; margin-bottom: 12px; }
.network-controls button, .settings-controls button {
    display: block;
    padding: 7px 9px;
    margin-right: 5px;
    margin-bottom: 5px;
    color: #e8e6e1;
    background: #262e3a;
    border: 1px solid #33405a;
}
.management-settings {
    display: block;
    width: 760px;
    padding: 12px 14px;
    margin-bottom: 18px;
    background: #171b23;
}
.settings-controls { display: flex; margin-bottom: 10px; }
.network-canvas { display: block; width: 760px; margin-bottom: 18px; }
.network-heading { display: block; font-size: 16px; margin: 14px 0 7px 0; }
.network-rows, .network-relations { display: block; width: 760px; }
.network-row, .network-relation {
    display: block;
    width: 760px;
    padding: 8px 10px;
    margin-bottom: 3px;
    color: #e8e6e1;
    background: #1c222c;
    border: 1px solid #2b3441;
    text-align: left;
}
.network-row.selected, .network-relation.selected {
    border-color: #a8c8ee;
    background: #263548;
}
";

pub fn sheet() -> String {
    format!("{SHEET}\n{}", cambium::GRAPH_CANVAS_SWATCH_CSS)
}
