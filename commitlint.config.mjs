// Conventional Commits: `type(scope): subject`, then an optional prose body.
// The body stays free-form prose, so its lines have no length limit.
export default {
  extends: ['@commitlint/config-conventional'],
  rules: {
    'body-max-line-length': [0],
    'footer-max-line-length': [0],
  },
};
