/** @type {import('tailwindcss').Config} */
export default {
  darkMode: "class",
  content: ["./index.html", "./src/**/*.{vue,ts,tsx}"],
  theme: {
    extend: {
      colors: {
        primary: {
          DEFAULT: "var(--gosslan-primary)",
          hover: "var(--gosslan-primary-hover)",
          active: "var(--gosslan-primary-active)",
          light: "var(--gosslan-primary-light)",
        },
      },
      fontFamily: {
        gosslan: "var(--gosslan-font-family, -apple-system, 'Segoe UI', 'PingFang SC', 'Microsoft YaHei', sans-serif)",
      },
    },
  },
  plugins: [],
};
