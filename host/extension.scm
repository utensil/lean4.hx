;; Load this file from a Helix init.scm after installing the cdylib into
;; STEEL_HOME/native, then call (lean4-hx-install!).
(require-builtin helix/core/editor as editor.)
(require-builtin helix/core/misc as hx.)
(require-builtin helix/core/static as static.)
(require-builtin helix/core/typable as command.)
(require-builtin helix/core/text as text.)
(require-builtin helix/components as components.)
(#%require-dylib "liblean4_hx"
  (only-in native-state native-activate! native-deactivate! native-active?
           native-record-selection! native-record-document-change! native-record-open!
           native-record-close! native-reset-rpc-after-server-restart! native-record-request! native-record-callback!
           native-record-hover! native-record-inlay! native-record-navigation! native-record-navigation-applied! native-record-signature!
           native-begin-request! native-cancel-request!
           native-generation native-generation-current? native-rpc-goals-request native-rpc-session-current? native-rpc-session-goals-request
           native-rpc-term-goal-request native-record-rpc-action!
           native-rpc-keepalive-request native-rpc-release-request native-record-rpc! native-label native-styled-lines native-summary native-file-uri
           native-focused? native-set-focused! native-scroll! native-next-goal! native-previous-goal!
           native-unicode-start native-unicode-end
           native-unicode-replacement native-unicode-cursor native-utf16-to-chars
           native-navigation-target native-navigation-path native-navigation-uri native-navigation-start-line
           native-navigation-start-character native-navigation-end-line
           native-navigation-end-character))

(provide lean4-hx-install!)
(provide lean4-hx-remove-component!)
(provide lean4-hx-request-lean-info!)
(provide lean4-hx-request-term-goal!)
(provide lean4-hx-code-action!)
(provide lean4-hx-summary)
(provide lean4-hx-native)

;; The value is opaque to Scheme. All counters and lifecycle state are Rust-owned.
(define lean4-hx-native (native-state))
(define lean4-hx-component #f)
(define lean4-hx-installed? #f)
(define lean4-hx-hooks-installed? #f)
(define lean4-hx-selection-pending? #f)
(define lean4-hx-last-file #f)
(define lean4-hx-panel-width 36)

;; Dynamic components are compositor layers, so Helix gives them the full
;; terminal rectangle. Reserve a right editor strip before rendering the
;; component; this keeps goal text in its own pane instead of painting over
;; the source buffer.
(define (lean4-hx-apply-panel-layout! width)
  (editor.set-editor-clip-right! width))

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
  ;; Helix does not emit document-opened when switching back to an already
  ;; loaded buffer. Detect that URI transition here so a Lean RPC session can
  ;; never be reused for a different document.
  (let ([path (static.cx->current-file)])
    (if (and path
             (or (not lean4-hx-last-file)
                 (not (string=? path lean4-hx-last-file))))
        (begin
          (lean4-hx-rpc-release!)
          (native-record-open! lean4-hx-native)
          (set! lean4-hx-last-file path))
        #f))
  (native-record-selection! lean4-hx-native)
  ;; Cancel immediately; the delayed callback reads the latest cursor.
  (if (native-active? lean4-hx-native)
      (native-cancel-request! lean4-hx-native)
      #f)
  (lean4-hx-schedule-refresh!))

(define (lean4-hx-on-document-change document old-text)
  (native-record-document-change! lean4-hx-native)
  (lean4-hx-schedule-refresh!))

(define (lean4-hx-on-insert-char char)
  (if (equal? char #\space)
      (lean4-hx-expand-unicode!)
      #f))

(define (lean4-hx-on-open document)
  ;; A document switch invalidates the Rust-owned cursor/session state. Release
  ;; the previous Lean RPC session before that state is replaced so switching
  ;; buffers cannot leak opaque references on the server.
  (lean4-hx-rpc-release!)
  (native-record-open! lean4-hx-native)
  (set! lean4-hx-last-file (static.cx->current-file))
  (lean4-hx-schedule-refresh!))

(define (lean4-hx-on-post-command command-name)
  ;; Helix does not expose a language-server restart hook. Reset the owned
  ;; Lean RPC session after the user invokes the restart command so the next
  ;; cursor request connects afresh instead of sending a stale session id.
  (if (string=? command-name "lsp-restart")
      (native-reset-rpc-after-server-restart! lean4-hx-native)
      #f))

(define (lean4-hx-rpc-keepalive!)
  (let ([request (string->jsexpr (native-rpc-keepalive-request lean4-hx-native))])
    (if (hash-contains? request 'sessionId)
        (begin
          (hx.send-lsp-notification "lean" "$/lean/rpc/keepAlive" request)
          #t)
        #f)))

(define (lean4-hx-rpc-release!)
  (let ([request (string->jsexpr (native-rpc-release-request lean4-hx-native))])
    (if (hash-contains? request 'sessionId)
        (begin
          ;; Teardown must not block a document-close hook when the language
          ;; server is being restarted or has already exited.
          (hx.enqueue-thread-local-callback-with-delay
           0
           (lambda ()
             (hx.send-lsp-notification "lean" "$/lean/rpc/release" request)))
          #t)
        #f)))

(define (lean4-hx-request-term-goal!)
  (let ([generation (native-generation lean4-hx-native)])
    (let* ([request (string->jsexpr
                     (native-rpc-term-goal-request lean4-hx-native generation))]
           [position (hash-ref request 'position)]
           ;; Steel may parse JSON numbers as inexact values. Lean's RPC
           ;; decoder requires Nat positions, so normalize both copies before
           ;; sending the request.
           [line (exact (hash-ref position 'line))]
           [character (exact (hash-ref position 'character))]
           [position (hash-insert
                      (hash-insert position 'line line)
                      'character character)]
           [params (hash-ref request 'params)]
           [params (hash-insert
                    (hash-insert params 'position position)
                    'textDocument (hash-ref request 'textDocument))]
           [request (hash-insert
                     (hash-insert request 'position position)
                     'params params)])
      (if (hash-contains? request 'sessionId)
          (hx.send-lsp-command
           "lean" "$/lean/rpc/call" request
           (lambda (result)
             (native-record-rpc-action! lean4-hx-native generation
                                        (value->jsexpr-string result))))
          (hx.set-warning! "lean4.hx: term goal unavailable")))))

(define (lean4-hx-on-close closed-event)
  (lean4-hx-rpc-release!)
  (native-record-close! lean4-hx-native))

(define (lean4-hx-on-lean-info generation result)
  (if (native-active? lean4-hx-native)
      (begin
        (native-record-callback! lean4-hx-native generation
                                 (value->jsexpr-string result)))
      #f))

(define (lean4-hx-on-definition generation result)
  (if (native-active? lean4-hx-native)
      (native-record-navigation! lean4-hx-native generation
                                (value->jsexpr-string result))
      #f))

(define (lean4-hx-on-hover generation result)
  (if (native-active? lean4-hx-native)
      (native-record-hover! lean4-hx-native generation
                            (value->jsexpr-string result))
      #f))

(define (lean4-hx-on-signature generation result)
  (if (native-active? lean4-hx-native)
      (native-record-signature! lean4-hx-native generation
                                (value->jsexpr-string result))
      #f))

(define (lean4-hx-on-inlay generation result)
  (if (native-active? lean4-hx-native)
      (native-record-inlay! lean4-hx-native generation
                            (value->jsexpr-string result))
      #f))

(define (lean4-hx-on-rpc generation result)
  (if (native-active? lean4-hx-native)
      (begin
        (native-record-rpc! lean4-hx-native generation
                            (value->jsexpr-string result))
        (lean4-hx-rpc-keepalive!)
        (let ([request (string->jsexpr
                        (native-rpc-goals-request lean4-hx-native generation
                                                  (value->jsexpr-string result)))])
          (if (hash-contains? request 'sessionId)
              (let* ([position (hash-ref request 'position)]
                     [line (exact (hash-ref position 'line))]
                     [character (exact (hash-ref position 'character))]
                     [position (hash-insert
                                (hash-insert position 'line line)
                                'character character)]
                     [params (hash-ref request 'params)]
                     [params (hash-insert
                              (hash-insert params 'position position)
                              'textDocument (hash-ref request 'textDocument))]
                     [request (hash-insert
                               (hash-insert request 'position position)
                               'params params)])
                (hx.enqueue-thread-local-callback-with-delay
                 0
                 (lambda ()
                   (if (native-generation-current? lean4-hx-native generation)
                       (hx.send-lsp-command
                        "lean" "$/lean/rpc/call" request
                        (lambda (reply)
                          (native-record-rpc! lean4-hx-native generation
                                               (value->jsexpr-string reply))))
                       #f))))
              #f)))
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
                  (hx.send-lsp-command "lean" "textDocument/hover" params
                                       (lambda (result)
                                         (lean4-hx-on-hover generation result)))
                  (hx.send-lsp-command "lean" "textDocument/signatureHelp" params
                                       (lambda (result)
                                         (lean4-hx-on-signature generation result)))
                  (hx.send-lsp-command "lean" "textDocument/inlayHint"
                                       (hash "textDocument" (hash "uri" uri)
                                             "range" (hash "start" (hash "line" line "character" character)
                                                       "end" (hash "line" line "character" (+ character 1))))
                                       (lambda (result)
                                         (lean4-hx-on-inlay generation result)))
                  (hx.send-lsp-command "lean" "textDocument/definition" params
                                       (lambda (result)
                                         (lean4-hx-on-definition generation result)))
                  (if (native-rpc-session-current? lean4-hx-native generation)
                      (let ([request
                             (string->jsexpr
                              (native-rpc-session-goals-request
                               lean4-hx-native generation))])
                        (if (hash-contains? request 'sessionId)
                            (let* ([position (hash-ref request 'position)]
                                   [line (exact (hash-ref position 'line))]
                                   [character (exact (hash-ref position 'character))]
                                   [position (hash-insert
                                              (hash-insert position 'line line)
                                              'character character)]
                                   [params (hash-ref request 'params)]
                                   [params (hash-insert
                                            (hash-insert params 'position position)
                                            'textDocument (hash-ref request 'textDocument))]
                                   [request (hash-insert
                                             (hash-insert request 'position position)
                                             'params params)])
                              (hx.send-lsp-command
                               "lean" "$/lean/rpc/call" request
                               (lambda (result)
                                 (lean4-hx-on-rpc generation result))))
                            #f))
                      (hx.send-lsp-command "lean" "$/lean/rpc/connect"
                                           (hash "uri" uri)
                                           (lambda (result)
                                             (lean4-hx-on-rpc generation result)))))
                #f)))
        (hx.set-warning! "lean4.hx: current document has no file URI"))))

;; Delegate ordinary Lean fixes to Helix's stock, version-aware code-action
;; picker.  The server resolves and applies the selected action through the
;; host's normal undo/history path.
(define (lean4-hx-code-action!)
  (static.code_action))

(define (lean4-hx-apply-navigation!)
  ;; Capture the whole target before opening: open/selection hooks invalidate it.
  (let* ([target-json (native-navigation-target lean4-hx-native)]
         [target (string->jsexpr target-json)])
    (if (not (hash-contains? target 'path))
        (hx.set-warning! "lean4.hx: navigation unavailable")
        (let ([path (hash-ref target 'path)]
              [start-line (exact (hash-ref target 'start-line))]
              [end-line (exact (hash-ref target 'end-line))]
              [start-character (exact (hash-ref target 'start-character))]
              [end-character (exact (hash-ref target 'end-character))])
        (begin
          (command.open path)
          (let* ([view (editor.editor-focus)]
                 [document (editor.editor->doc-id view)]
                 [rope (editor.editor->text document)]
                 [start-rope (and (< start-line (text.rope-len-lines rope))
                                  (text.rope->line rope start-line))]
                 [end-rope (and (< end-line (text.rope-len-lines rope))
                                (text.rope->line rope end-line))])
            (if (and start-rope end-rope)
                (let* ([start-text (text.rope->string start-rope)]
                       [end-text (text.rope->string end-rope)]
                       [start (+ (text.rope-line->char rope start-line)
                                 (native-utf16-to-chars lean4-hx-native start-text start-character))]
                       [end (+ (text.rope-line->char rope end-line)
                               (native-utf16-to-chars lean4-hx-native end-text end-character))])
                  (static.set-current-selection-object!
                   (static.range->selection (static.range start end)))
                  (native-set-focused! lean4-hx-native #f)
                  (native-record-navigation-applied! lean4-hx-native path start-line start-character)
                  (hx.set-status! "lean4.hx: navigated"))
                (hx.set-warning! "lean4.hx: navigation target range unavailable"))))))))

(define (lean4-hx-render state area frame)
  (let* ([width (min lean4-hx-panel-width (components.area-width area))]
         [left (+ (components.area-x area)
                  (- (components.area-width area) width))]
         [panel (components.area left
                                 (components.area-y area)
                                 width
                                 (components.area-height area))]
         [row (components.area-y panel)]
         [last-row (+ (components.area-y panel)
                      (components.area-height panel))])
    ;; Clear the reserved rectangle before drawing shorter goal responses.
    (components.buffer/clear-with frame panel (components.style))
    (let ([styled (string->jsexpr
                   (native-styled-lines state (components.area-width panel)))])
      (for-each
       (lambda (line)
         (if (< row last-row)
             (for-each
              (lambda (span)
                (let* ([style-name (hash-ref span 'style)]
                       [style (cond
                               [(string=? style-name "goal")
                                (components.style-fg
                                 (components.style-with-bold (components.style))
                                 components.Color/Blue)]
                               [(string=? style-name "type")
                                (components.style-fg (components.style) components.Color/Cyan)]
                               [(string=? style-name "keyword")
                                (components.style-fg (components.style) components.Color/Yellow)]
                               [(string=? style-name "error")
                                (components.style-fg (components.style) components.Color/Red)]
                               [else (components.style)])])
                  (components.frame-set-string!
                   frame (+ (components.area-x panel) (exact (hash-ref span 'column)))
                   row (hash-ref span 'text) style)))
              line)
             #f)
        (set! row (+ row 1)))
       styled))))

(define (lean4-hx-handle-event state event)
  (cond
   [(components.key-event-tab? event)
    (native-set-focused! state #t)
    components.event-result/consume]
   [(components.key-event-escape? event)
    (native-set-focused! state #f)
    ;; Let Helix also process Escape so insert mode returns to normal mode.
    components.event-result/ignore]
   [(not (native-focused? state)) components.event-result/ignore]
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
   [(and (components.key-event-char event)
         (equal? (components.key-event-char event) #\t))
    (lean4-hx-request-term-goal!)
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
              (editor.register-hook 'post-command lean4-hx-on-post-command)
              (set! lean4-hx-hooks-installed? #t))
            #f)
        (set! lean4-hx-component
              (components.new-component! "lean4-hx-component" lean4-hx-native
                                          lean4-hx-render
                                          (hash "handle_event"
                                                lean4-hx-handle-event)))
        (lean4-hx-apply-panel-layout! lean4-hx-panel-width)
        (hx.push-component! lean4-hx-component)
        (set! lean4-hx-installed? #t)
        "lean4.hx installed")))

(define (lean4-hx-remove-component!)
  (lean4-hx-rpc-release!)
  (if lean4-hx-component
      (begin
        (hx.pop-last-component-by-name! "lean4-hx-component")
        (set! lean4-hx-component #f))
      #f)
  (native-deactivate! lean4-hx-native)
  (lean4-hx-apply-panel-layout! 0)
  (set! lean4-hx-installed? #f)
  (set! lean4-hx-selection-pending? #f)
  (set! lean4-hx-last-file #f)
  "lean4.hx component removed")
