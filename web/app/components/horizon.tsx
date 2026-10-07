// The sun rising over Long Island Sound, with a constellation of connected
// points in the sky for the AI half of the name.
const nodes = [
  [92, 70],
  [150, 46],
  [214, 82],
  [276, 52],
  [330, 96],
  [178, 128],
  [256, 138],
] as const;

const links = [
  [0, 1],
  [1, 2],
  [2, 3],
  [3, 4],
  [1, 5],
  [2, 5],
  [2, 6],
  [3, 6],
] as const;

export function Horizon() {
  return (
    <svg className="horizon" viewBox="0 0 420 420" aria-hidden="true" focusable="false">
      <defs>
        <clipPath id="horizon-frame">
          <rect width="420" height="420" rx="210" />
        </clipPath>
      </defs>
      <g clipPath="url(#horizon-frame)">
        <rect className="horizon-sky" width="420" height="420" />
        <g className="horizon-links">
          {links.map(([a, b]) => (
            <line
              key={`${a}-${b}`}
              x1={nodes[a][0]}
              y1={nodes[a][1]}
              x2={nodes[b][0]}
              y2={nodes[b][1]}
            />
          ))}
        </g>
        <g className="horizon-nodes">
          {nodes.map(([x, y]) => (
            <circle key={`${x}-${y}`} cx={x} cy={y} r="5" />
          ))}
        </g>
        <circle className="horizon-sun" cx="210" cy="262" r="74" />
        <path
          className="horizon-water-back"
          d="M0 262c35-14 70-14 105 0s70 14 105 0 70-14 105 0 70 14 105 0v158H0z"
        />
        <path
          className="horizon-water"
          d="M0 300c35-12 70-12 105 0s70 12 105 0 70-12 105 0 70 12 105 0v120H0z"
        />
        <path
          className="horizon-water-front"
          d="M0 346c35-10 70-10 105 0s70 10 105 0 70-10 105 0 70 10 105 0v74H0z"
        />
      </g>
    </svg>
  );
}
