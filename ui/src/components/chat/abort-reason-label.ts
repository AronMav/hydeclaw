export function abortReasonLabel(reason: string | null | undefined): string {
  switch (reason) {
    case "max_duration":
      return "Aborted — exceeded stream_max_duration_secs.";
    case "inactivity":
      return "Provider stopped sending data.";
    case "user_cancelled":
      return "Stopped by you.";
    case "shutdown_drain":
      return "Interrupted by service restart.";
    case "connect_timeout":
    case "request_timeout":
      return "Timed out — retrying next provider.";
    default:
      return reason ? `Aborted (${reason}).` : "Aborted.";
  }
}
