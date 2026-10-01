;; Load this file from a Helix init.scm after installing the cdylib into
;; STEEL_HOME/native, then call (lean4-hx-install!).
(require-builtin helix/core/editor as editor.)
(require-builtin helix/core/misc as hx.)
(require-builtin helix/core/static as static.)
(require-builtin helix/core/text as text.)
(require-builtin helix/components as components.)
(#%require-dylib "liblean4_hx"
  (only-in native-state native-activate! native-deactivate! native-active?
           native-record-selection! native-record-document-change! native-record-open!
           native-record-close! native-record-request! native-record-callback!
           native-record-navigation! native-begin-request! native-cancel-request!
           native-record-rpc! native-label native-lines native-summary native-file-uri
           native-set-focused! native-scroll! native-next-goal! native-previous-goal!
           native-unicode-start native-unicode-end
           native-unicode-replacement native-unicode-cursor native-utf16-to-chars
           native-navigation-uri native-navigation-start-line
           native-navigation-start-character native-navigation-end-line
           native-navigation-end-character))

(provide lean4-hx-install!)
(provide lean4-hx-remove-component!)
(provide lean4-hx-request-lean-info!)
(provide lean4-hx-summary)
(provide lean4-hx-native)

;; The value is opaque to Scheme. All counters and lifecycle state are Rust-owned.
(define lean4-hx-native (native-state))
(define lean4-hx-component #f)
(define lean4-hx-installed? #f)
(define lean4-hx-hooks-installed? #f)
(define lean4-hx-selection-pending? #f)

(define (lean4-hx-expand-unicode!)
  (let ([path (static.cx->current-file)])
    (if path
        (let* ([view (editor.editor-focus)]
               [document (editor.editor->doc-id view)]
               [rope (editor.editor->text document)]
               [line-number (static.get-current-line-number)]
               [line (text.rope->string (text.rope->line rope line-number))]
               [cursor (static.get-current-line-character "utf-16")]
               [start (native-unicode-start lean4-hx-native line cursor)]
               [end (native-unicode-end lean4-hx-native line cursor)]
               [replacement (native-unicode-replacement lean4-hx-native line cursor)]
               [cursor-offset (native-unicode-cursor lean4-hx-native line cursor)])
          (if (and (> end start) (not (string=? replacement "")))
              (let ([line-start (text.rope-line->char rope line-number)])
                (static.set-current-selection-object!
                 (static.range->selection
                  (static.range (+ line-start start) (+ line-start end))))
                (static.replace-selection-with replacement)
                ;; Paired abbreviations carry a cursor marker. Re-read the
                ;; line start after replacement and collapse the selection at
                ;; that marker instead of leaving the cursor after the pair.
                (static.set-current-selection-object!
                 (static.range->selection
                  (static.range (+ line-start start cursor-offset)
                                (+ line-start start cursor-offset))))
                (lean4-hx-status "lean4.hx unicode"))
              #f))
        #f)))

(define (lean4-hx-summary)
  (native-summary lean4-hx-native))

(define (lean4-hx-status prefix)
  (hx.set-status! (string-append prefix ": " (lean4-hx-summary))))

(define (lean4-hx-schedule-refresh!)
  ;; Coalesce open, edit, and selection events.  The callback is guarded by
  ;; native-active? so a close/remove cannot resurrect a request.
  (if (and (native-active? lean4-hx-native)
           (not lean4-hx-selection-pending?))
      (begin
        (set! lean4-hx-selection-pending? #t)
        (hx.enqueue-thread-local-callback-with-delay
         500
         (lambda ()
           (set! lean4-hx-selection-pending? #f)
           (if (native-active? lean4-hx-native)
               (begin
                 (native-cancel-request! lean4-hx-native)
                 (lean4-hx-request-lean-info!))
               #f))))
      #f))

(define (lean4-hx-on-selection view)
  (native-record-selection! lean4-hx-native)
  ;; Cancel immediately; the delayed callback reads the latest cursor.
  (if (native-active? lean4-hx-native)
      (native-cancel-request! lean4-hx-native)
      #f)
  (lean4-hx-schedule-refresh!)
  (lean4-hx-status "lean4.hx selection"))

(define (lean4-hx-on-document-change document old-text)
  (native-record-document-change! lean4-hx-native)
  (lean4-hx-schedule-refresh!))

(define (lean4-hx-on-insert-char char)
  (if (equal? char #\space)
      (lean4-hx-expand-unicode!)
      #f))

(define (lean4-hx-on-open document)
  (native-record-open! lean4-hx-native)
  (lean4-hx-schedule-refresh!))

(define (lean4-hx-on-close closed-event)
  (native-record-close! lean4-hx-native))

(define (lean4-hx-on-lean-info generation result)
  (if (native-active? lean4-hx-native)
      (begin
        (native-record-callback! lean4-hx-native generation
                                 (value->jsexpr-string result))
        (lean4-hx-status "lean4.hx Lean callback"))
      #f))

(define (lean4-hx-on-definition generation result)
  (if (native-active? lean4-hx-native)
      (native-record-navigation! lean4-hx-native generation
                                (value->jsexpr-string result))
      #f))

(define (lean4-hx-on-rpc generation result)
  (if (native-active? lean4-hx-native)
      (native-record-rpc! lean4-hx-native generation
                          (value->jsexpr-string result))
      #f))

(define (lean4-hx-request-lean-info!)
  (let ([path (static.cx->current-file)])
    (if path
        (let* ([line (static.get-current-line-number)]
               [character (static.get-current-line-character "utf-16")]
               [uri (native-file-uri path)]
               [params (hash "textDocument" (hash "uri" uri)
                             "position" (hash "line" line
                                               "character" character))])
          (native-record-request! lean4-hx-native path line character)
          (let ([generation (native-begin-request! lean4-hx-native path line character)])
            (if (> generation 0)
                (begin
                  (hx.send-lsp-command "lean" "$/lean/plainGoal" params
                                       (lambda (result)
                                         (lean4-hx-on-lean-info generation result)))
                  (hx.send-lsp-command "lean" "textDocument/definition" params
                                       (lambda (result)
                                         (lean4-hx-on-definition generation result)))
                  (hx.send-lsp-command "lean" "$/lean/rpc/connect"
                                       (hash "uri" uri)
                                       (lambda (result)
                                         (lean4-hx-on-rpc generation result))))
                #f))
          (lean4-hx-status "lean4.hx Lean request"))
        (hx.set-warning! "lean4.hx: current document has no file URI"))))

(define (lean4-hx-apply-navigation!)
  (let ([path (static.cx->current-file)]
        [uri (native-navigation-uri lean4-hx-native)])
    (if (and path (not (string=? uri "")))
        (if (string=? uri (native-file-uri path))
            (let* ([view (editor.editor-focus)]
                   [document (editor.editor->doc-id view)]
                   [rope (editor.editor->text document)]
                   [start-line (native-navigation-start-line lean4-hx-native)]
                   [end-line (native-navigation-end-line lean4-hx-native)]
                   [start-text (text.rope->string (text.rope->line rope start-line))]
                   [end-text (text.rope->string (text.rope->line rope end-line))]
                   [start-column (native-utf16-to-chars
                                 lean4-hx-native start-text
                                 (native-navigation-start-character lean4-hx-native))]
                   [end-column (native-utf16-to-chars
                               lean4-hx-native end-text
                               (native-navigation-end-character lean4-hx-native))]
                   [start (+ (text.rope-line->char rope start-line) start-column)]
                   [end (+ (text.rope-line->char rope end-line) end-column)])
              (static.set-current-selection-object!
               (static.range->selection (static.range start end)))
              (lean4-hx-status "lean4.hx navigated"))
            (hx.set-warning! "lean4.hx: cross-file navigation needs host open support"))
        (hx.set-warning! "lean4.hx: navigation unavailable"))))

(define (lean4-hx-render state area frame)
  (let ([row (components.area-y area)]
        [last-row (+ (components.area-y area) (components.area-height area))])
    (for-each
     (lambda (line)
       (if (< row last-row)
           (components.frame-set-string! frame
                                         (components.area-x area)
                                         row
                                         line
                                         (components.style)))
       (set! row (+ row 1)))
     (native-lines state))))

(define (lean4-hx-handle-event state event)
  (cond
   [(components.key-event-tab? event)
    (native-set-focused! state #t)
    components.event-result/consume]
   [(components.key-event-escape? event)
    (native-set-focused! state #f)
    ;; Let Helix also process Escape so insert mode returns to normal mode.
    components.event-result/ignore]
   [(components.key-event-page-down? event)
    (native-scroll! state 1)
    components.event-result/consume]
   [(components.key-event-page-up? event)
    (native-scroll! state -1)
    components.event-result/consume]
   [(and (components.key-event-char event)
         (equal? (components.key-event-char event) #\g))
    (lean4-hx-apply-navigation!)
    components.event-result/consume]
   [(and (components.key-event-char event)
         (equal? (components.key-event-char event) #\n))
    (native-next-goal! state)
    components.event-result/consume]
   [(and (components.key-event-char event)
         (equal? (components.key-event-char event) #\p))
    (native-previous-goal! state)
    components.event-result/consume]
   [else components.event-result/ignore]))

(define (lean4-hx-install!)
  (if lean4-hx-installed?
      "lean4.hx already installed"
      (begin
        ;; Removing by name also clears a component left behind by a previous
        ;; init.scm reload. Hooks are process-scoped in Helix, so register them
        ;; only once; otherwise remove/reinstall would duplicate callbacks.
        (hx.pop-last-component-by-name! "lean4-hx-component")
        (set! lean4-hx-selection-pending? #f)
        (native-activate! lean4-hx-native)
        (if (not lean4-hx-hooks-installed?)
            (begin
              (editor.register-hook 'selection-did-change lean4-hx-on-selection)
              (editor.register-hook 'post-insert-char lean4-hx-on-insert-char)
              (editor.register-hook 'document-changed lean4-hx-on-document-change)
              (editor.register-hook 'document-opened lean4-hx-on-open)
              (editor.register-hook 'document-closed lean4-hx-on-close)
              (set! lean4-hx-hooks-installed? #t))
            #f)
        (set! lean4-hx-component
              (components.new-component! "lean4-hx-component" lean4-hx-native
                                          lean4-hx-render
                                          (hash "handle_event"
                                                lean4-hx-handle-event)))
        (hx.push-component! lean4-hx-component)
        (set! lean4-hx-installed? #t)
        (lean4-hx-status "lean4.hx installed")
        "lean4.hx installed")))

(define (lean4-hx-remove-component!)
  (if lean4-hx-component
      (begin
        (hx.pop-last-component-by-name! "lean4-hx-component")
        (set! lean4-hx-component #f))
      #f)
  (native-deactivate! lean4-hx-native)
  (set! lean4-hx-installed? #f)
  (set! lean4-hx-selection-pending? #f)
  (lean4-hx-status "lean4.hx removed")
  "lean4.hx component removed")
