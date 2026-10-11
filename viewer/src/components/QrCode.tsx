/** Renders QR modules from the backend (row-major, true = dark) as an SVG. */
export default function QrCode({ width, modules, label }: { width: number; modules: boolean[]; label: string }) {
  const quiet = 4;
  const size = width + quiet * 2;
  let path = "";
  modules.forEach((dark, index) => {
    if (dark) path += `M${(index % width) + quiet} ${Math.floor(index / width) + quiet}h1v1h-1z`;
  });
  return (
    <svg className="qr-code" viewBox={`0 0 ${size} ${size}`} role="img" aria-label={label} shapeRendering="crispEdges">
      <rect width={size} height={size} fill="#fff" />
      <path d={path} fill="#000" />
    </svg>
  );
}
