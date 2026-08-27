/**
 * Centralized theme constants for easy customization
 * 
 * Light mode uses semi-transparent backgrounds for glassmorphism effect
 * Dark mode uses lower opacity for better contrast with dark backgrounds
 */

export const themeColors = {
  // Light mode colors
  light: {
    // Main backgrounds
    pageBackground: 'from-slate-50 via-slate-100/80 to-slate-50',
    
    // Card/Surface backgrounds (balanced between transparent and solid)
    cardBackground: 'bg-white/85',
    cardBorder: 'border-slate-200/80',
    
    // Secondary surfaces (slightly less opaque)
    surfaceBackground: 'bg-slate-50/90',
    surfaceBorder: 'border-slate-200/70',
    
    // Interactive elements
    buttonBackground: 'bg-white/90',
    buttonBorder: 'border-slate-200/80',
    buttonHoverBackground: 'hover:bg-white',
    buttonHoverBorder: 'hover:border-slate-300',
    
    // Mode selector / List items
    itemBackground: 'bg-slate-50/80',
    itemBorder: 'border-slate-200/70',
    itemHoverBackground: 'hover:bg-slate-100/80',
    itemHoverBorder: 'hover:border-slate-300/80',
  },
  
  // Dark mode colors (keep existing)
  dark: {
    pageBackground: 'dark:from-[#101012] dark:via-[#141418] dark:to-[#0f0f12]',
    
    cardBackground: 'dark:bg-white/10',
    cardBorder: 'dark:border-white/10',
    
    surfaceBackground: 'dark:bg-white/5',
    surfaceBorder: 'dark:border-white/10',
    
    buttonBackground: 'dark:bg-white/10',
    buttonBorder: 'dark:border-white/10',
    buttonHoverBackground: 'dark:hover:bg-white/15',
    buttonHoverBorder: 'dark:hover:border-white/20',
    
    itemBackground: 'dark:bg-white/5',
    itemBorder: 'dark:border-white/10',
    itemHoverBackground: 'dark:hover:bg-white/10',
    itemHoverBorder: 'dark:hover:border-white/20',
  }
};

/**
 * Helper to combine light and dark mode classes
 */
export const getThemeClass = (lightKey, darkKey = lightKey) => {
  return `${themeColors.light[lightKey]} ${themeColors.dark[darkKey]}`;
};

/**
 * Pre-built combinations for common patterns
 */
export const themeClasses = {
  // Main page background
  pageBackground: `${themeColors.light.pageBackground} ${themeColors.dark.pageBackground}`,
  
  // Card/Panel (main surfaces like header, sections)
  card: `${themeColors.light.cardBackground} ${themeColors.light.cardBorder} ${themeColors.dark.cardBackground} ${themeColors.dark.cardBorder}`,
  
  // Secondary surface (nested elements)
  surface: `${themeColors.light.surfaceBackground} ${themeColors.light.surfaceBorder} ${themeColors.dark.surfaceBackground} ${themeColors.dark.surfaceBorder}`,
  
  // Button/Interactive element
  button: `${themeColors.light.buttonBackground} ${themeColors.light.buttonBorder} ${themeColors.light.buttonHoverBackground} ${themeColors.light.buttonHoverBorder} ${themeColors.dark.buttonBackground} ${themeColors.dark.buttonBorder} ${themeColors.dark.buttonHoverBackground} ${themeColors.dark.buttonHoverBorder}`,
  
  // List item / Mode selector item
  item: `${themeColors.light.itemBackground} ${themeColors.light.itemBorder} ${themeColors.light.itemHoverBackground} ${themeColors.light.itemHoverBorder} ${themeColors.dark.itemBackground} ${themeColors.dark.itemBorder} ${themeColors.dark.itemHoverBackground} ${themeColors.dark.itemHoverBorder}`,
};
