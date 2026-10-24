//! Documents and data the agent produces (`app/common/{mdhtml,xlsx,docx_writer,dataq,charts}.py`):
//! Markdown → HTML, Excel and Word writers, the `data_query` engine and chart HTML.

pub mod charts;
pub mod dataq;
pub mod docx_writer;
pub mod mdhtml;
pub mod xlsx;

mod expr;
mod pycsv;
mod pyfmt;
