import typography from "@tailwindcss/typography";

/** @type {import('tailwindcss').Config} */
export default {
  content: ["./index.html", "./src/**/*.{ts,tsx}"],
  theme: {
    extend: {
      colors: {
        bg: "#0e1116",
        panel: "#161b22",
        border: "#30363d",
        muted: "#8b949e",
      },
    },
  },
  plugins: [typography],
};
