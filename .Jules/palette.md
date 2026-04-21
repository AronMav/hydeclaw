## 2026-04-21 - [Micro-UX] ApprovalCard Loading States & Accessibility

**Learning:** Interactive elements like the "Approve" and "Reject" buttons in `ApprovalCard` lacked visual feedback during async operations, which could lead to multiple clicks or confusion. Additionally, the icon-only collapsible trigger for tool input was not accessible to screen readers.

**Action:** Added `Loader2` spinners to buttons during `isSubmitting` state and provided an `aria-label` to the collapsible trigger. Removed redundant `tabIndex={0}` to follow standard HTML behavior.
