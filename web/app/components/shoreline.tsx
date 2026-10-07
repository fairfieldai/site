// Long Island Sound along the bottom of every page.
export function Shoreline() {
  return (
    <svg
      className="shoreline"
      viewBox="0 0 1440 160"
      preserveAspectRatio="none"
      aria-hidden="true"
      focusable="false"
    >
      <path
        className="wave wave-back"
        d="M0 72c120-28 240-28 360 0s240 28 360 0 240-28 360 0 240 28 360 0v88H0z"
      />
      <path
        className="wave wave-front"
        d="M0 104c120-24 240-24 360 0s240 24 360 0 240-24 360 0 240 24 360 0v56H0z"
      />
    </svg>
  );
}
