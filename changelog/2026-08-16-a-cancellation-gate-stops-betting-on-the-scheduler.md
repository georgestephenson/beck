- **2026-08-16 — A cancellation gate stops betting on the scheduler.**
  `concurrency.rs::a_sibling_blocked_in_an_outbound_call_is_stopped_in_the_call` asserts that a
  scope reaches a child *blocked in the host*, and what put the sibling inside its call was
  arithmetic — twenty fast fetches first, on the reasoning that this made it "provably inside".
  Nothing enforced it, so under load the sibling was cancelled by the step counter before it ever
  entered a call and the test failed on its own guard while cancellation was working. The host now
  **holds** the failing child's first call until the sibling is blocked (a condvar, with a backstop
  that goes red rather than hanging), and the sibling has 4,000 steps to take before its fetch — so
  the hazard is exercised every run rather than only on a busy machine. Checked both ways: with the
  latch removed the test fails deterministically with the message that was seen intermittently, and
  it passes with it. Deletes `DEFECTS.md::blocked-sibling-race`;
  [`docs/80`](../docs/80-structured-concurrency-report.md) §80.14 is the property it guards.
