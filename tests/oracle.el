;;; oracle.el --- ground-truth HTML export for the org-ssg differential tests  -*- lexical-binding: t -*-

;; Exports the org file named by $ORG_ORACLE_INPUT to HTML on stdout, using org's own
;; exporter — the same one weblorg wraps to publish the corpus this project targets.
;; Run with:  ORG_ORACLE_INPUT=x.org emacs -Q --batch -l tests/oracle.el
;;
;; `-Q' is deliberate: no user init, so the oracle is the stock org exporter and not
;; this machine's Emacs configuration. The path is passed by environment variable
;; rather than as an argument because batch Emacs would otherwise try to visit it.

(require 'org)
(require 'ox-html)

;; Presentation settings are normalized so the diff carries semantic divergences only.
;; Everything that affects *content* is left at its default, because the point is to
;; learn what stock org does — normalizing that away would be marking our own homework.
(setq org-export-with-toc nil              ; we emit no table of contents
      org-export-with-section-numbers nil  ; we do not number headings
      ;; org-html-toplevel-hlevel is left at its default of 2. org-ssg's own default
      ;; heading_offset is 1, which produces the same <h2>, so both sides now agree
      ;; without the oracle being told to.
      org-html-htmlize-output-type nil     ; plain <pre>, not htmlize spans: we highlight
                                           ; with syntect, so comparing code *text* is
                                           ; the meaningful part
      org-html-head-include-default-style nil
      org-html-head-include-scripts nil
      ;; Fixtures link to ids that live in org-ssg's own symbol table, not in an
      ;; `org-id' database. Without this, org aborts the whole export on the first one.
      org-export-with-broken-links t
      make-backup-files nil)

(let ((input (getenv "ORG_ORACLE_INPUT")))
  (unless input
    (error "ORG_ORACLE_INPUT is not set"))
  (with-temp-buffer
    (insert-file-contents input)
    (org-mode)
    ;; BODY-ONLY: emit the content, not a full document with <head> chrome.
    (princ (org-export-as 'html nil nil t nil))))

;;; oracle.el ends here
