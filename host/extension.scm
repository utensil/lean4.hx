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
           native-record-close! native-reset-rpc-after-server-restart! native-record-request! native-record-callback! native-record-callback-with-completion!
           native-record-hover! native-record-inlay! native-record-navigation! native-record-navigation-applied! native-record-signature!
           native-begin-request! native-cancel-request!
           native-generation native-generation-current? native-rpc-goals-request native-rpc-session-current? native-rpc-session-goals-request
           native-rpc-term-goal-request native-record-rpc-action!
           native-rpc-keepalive-request native-rpc-release-request native-record-rpc! native-record-rpc-with-completion! native-label native-styled-lines native-info-lines native-selected-text native-selected-info-text native-summary native-file-uri
           native-goal-line-count native-focused? native-set-focused! native-scroll! native-next-goal! native-previous-goal!
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
(provide lean4-hx-set-layout!)
(provide lean4-hx-set-leader!)

;; The value is opaque to Scheme. All counters and lifecycle state are Rust-owned.
(define lean4-hx-native (native-state))
(define lean4-hx-component #f)
(define lean4-hx-installed? #f)
(define lean4-hx-hooks-installed? #f)
(define lean4-hx-selection-pending? #f)
(define lean4-hx-last-file #f)
(define lean4-hx-session-document #f)
(define lean4-hx-panel-width 36)
(define lean4-hx-goal-area #f)
(define lean4-hx-info-area #f)
(define lean4-hx-layout-mode "split")
(define lean4-hx-leader-label "Space l")
(define lean4-hx-leader-key #\l)
(define lean4-hx-prefix-pending? #f)
(define lean4-hx-leader-pending? #f)
(define lean4-hx-help-visible? #f)
(define lean4-hx-selection-start #f)
(define lean4-hx-selection-end #f)
(define lean4-hx-selection-info? #f)

(define (lean4-hx-clear-panel-focus!)
  ;; The compositor receives key events before Helix's editor keymap. Clear
  ;; panel focus whenever the editor regains ownership so source insertion can
  ;; never be shadowed by panel shortcuts.
  (native-set-focused! lean4-hx-native #f)
  (set! lean4-hx-prefix-pending? #f)
  (set! lean4-hx-leader-pending? #f)
  (set! lean4-hx-selection-start #f)
  (set! lean4-hx-selection-end #f)
  (set! lean4-hx-selection-info? #f))

(define (lean4-hx-on-mode-switch switch-event)
  ;; Match lean.nvim's window-local InfoView mappings: entering source insert
  ;; mode always returns key ownership to the editor, even if the panel had
  ;; focus immediately before the mode switch.
  (if (equal? (hx.mode-switch-new switch-event)
              (editor.string->editor-mode "insert"))
      (lean4-hx-clear-panel-focus!)
      #f))

(define (lean4-hx-set-layout! mode)
  (if (or (equal? mode "split") (equal? mode "adaptive"))
      (begin
        (set! lean4-hx-layout-mode mode)
        mode)
      "lean4.hx: layout must be split or adaptive"))

(define (lean4-hx-set-leader! label key)
  (if (and (string? label) (char? key))
      (begin
        (set! lean4-hx-leader-label label)
        (set! lean4-hx-leader-key key)
        label)
      "lean4.hx: leader requires a label string and key character"))

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

(define (lean4-hx-current-has-errors?)
  ;; An empty goal list can solve one bullet while sibling goals remain.  Do
  ;; not celebrate an empty result that arrived with a document error (for
  ;; example `apply h`).
  (let* ([view (editor.editor-focus)]
         [document (editor.editor->doc-id view)]
         [counts (editor.editor-document-diagnostic-counts document)])
    (and counts (> (list-ref counts 3) 0))))

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
  (lean4-hx-clear-panel-focus!)
  (native-record-selection! lean4-hx-native)
  ;; Cancel immediately; the delayed callback reads the latest cursor.
  (if (native-active? lean4-hx-native)
      (native-cancel-request! lean4-hx-native)
      #f)
  (lean4-hx-schedule-refresh!))

(define (lean4-hx-on-document-change document old-text)
  (lean4-hx-clear-panel-focus!)
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
  (lean4-hx-clear-panel-focus!)
  (lean4-hx-rpc-release!)
  (native-record-open! lean4-hx-native)
  (set! lean4-hx-last-file (static.cx->current-file))
  (lean4-hx-schedule-refresh!))

(define (lean4-hx-on-focus-lost document)
  ;; This hook carries the document that lost focus, so release through that
  ;; document's language-server client before a new buffer becomes current.
  (lean4-hx-clear-panel-focus!)
  (if (and lean4-hx-session-document
           (= (editor.doc-id->usize document)
              (editor.doc-id->usize lean4-hx-session-document)))
      (lean4-hx-rpc-release-for-document! document)
      #f))

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

(define (lean4-hx-rpc-release-for-document! document)
  (let ([request (string->jsexpr (native-rpc-release-request lean4-hx-native))])
    (if (and document (hash-contains? request 'sessionId))
        (with-handler
          (lambda (_) #f)
          (begin
            (hx.send-lsp-notification-for-document
             document "lean" "$/lean/rpc/release" request)
            (set! lean4-hx-session-document #f)
            #t))
        #f)))

(define (lean4-hx-rpc-release!)
  (lean4-hx-rpc-release-for-document! lean4-hx-session-document))

(define (lean4-hx-send-term-goal! generation)
  (let ([request (string->jsexpr
                  (native-rpc-term-goal-request lean4-hx-native generation))])
    (if (hash-contains? request 'position)
        (let* ([position (hash-ref request 'position)]
               ;; Steel may parse JSON numbers as inexact values. Lean's RPC
               ;; decoder requires Nat positions, so normalize both copies
               ;; before sending the request.
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
              (hx.set-status! "lean4.hx: term goal unavailable")))
        ;; A cursor refresh may have completed before the RPC connect callback.
        ;; Reconnect once, then retry against the same generation; this avoids
        ;; the intermittent unavailable action without retaining stale sessions.
        (let ([path (static.cx->current-file)])
          (if path
              (hx.send-lsp-command
               "lean" "$/lean/rpc/connect"
               (hash "uri" (native-file-uri path))
               (lambda (result)
                 (let ([wire (value->jsexpr-string result)])
                   (native-record-rpc! lean4-hx-native generation wire)
                   (if (hash-contains? (string->jsexpr wire) 'sessionId)
                       (lean4-hx-send-term-goal! generation)
                       (hx.set-status! "lean4.hx: term goal unavailable")))))
              (hx.set-status! "lean4.hx: term goal unavailable"))))))

(define (lean4-hx-request-term-goal!)
  (lean4-hx-send-term-goal! (native-generation lean4-hx-native)))

(define (lean4-hx-on-close closed-event)
  (lean4-hx-rpc-release-for-document! (editor.doc-closed-id closed-event))
  (native-record-close! lean4-hx-native))

(define (lean4-hx-on-lean-info generation result)
  (if (native-active? lean4-hx-native)
      (begin
        (native-record-callback-with-completion!
         lean4-hx-native generation (value->jsexpr-string result)
         (not (lean4-hx-current-has-errors?))))
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
                          (native-record-rpc-with-completion!
                           lean4-hx-native generation
                           (value->jsexpr-string reply)
                           (not (lean4-hx-current-has-errors?)))))
                       #f))))
              #f)))
      #f))

(define (lean4-hx-lean-ready?)
  (let ([ready #f])
    (for-each
     (lambda (client)
       (if (and (string=? (hx.lsp-client-name client) "lean")
                (hx.lsp-client-initialized? client))
           (set! ready #t)
           #f))
     (hx.get-active-lsp-clients))
    ready))

(define (lean4-hx-request-lean-info!)
  (let ([path (static.cx->current-file)])
    (if (and path (lean4-hx-lean-ready?))
        (let* ([view (editor.editor-focus)]
               [document (editor.editor->doc-id view)]
               [line (static.get-current-line-number)]
               [character (static.get-current-line-character "utf-16")]
               [uri (native-file-uri path)]
               [params (hash "textDocument" (hash "uri" uri)
                             "position" (hash "line" line
                                               "character" character))])
          (native-record-request! lean4-hx-native path line character)
          (let ([generation (native-begin-request! lean4-hx-native path line character)])
            (if (> generation 0)
                (begin
                  (set! lean4-hx-session-document document)
                  (hx.send-lsp-command "lean" "$/lean/plainGoal" params
                                       (lambda (result)
                                         (lean4-hx-on-lean-info generation result)))
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

(define (lean4-hx-render-lines frame panel styled)
  (let* ([row (components.area-y panel)]
         [last-row (+ (components.area-y panel)
                      (components.area-height panel))])
    (for-each
     (lambda (line)
       (if (< row last-row)
           (let ([selected? (and lean4-hx-selection-start
                                 lean4-hx-selection-end
                                 (<= (min lean4-hx-selection-start lean4-hx-selection-end) row)
                                 (<= row (max lean4-hx-selection-start lean4-hx-selection-end)))])
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
                               [else (components.style)])]
                       [style (if selected?
                                  (components.style-with-reversed style)
                                  style)])
                  (components.frame-set-string!
                   frame (+ (components.area-x panel) (exact (hash-ref span 'column)))
                   row (hash-ref span 'text) style)))
              line))
           #f)
      (set! row (+ row 1)))
     styled)))

(define (lean4-hx-render-panel-text! frame panel row text style)
  (if (< row (components.area-height panel))
      (components.frame-set-string!
       frame
       (+ (components.area-x panel) 1)
       (+ (components.area-y panel) row)
       text
       style)
      #f))

(define (lean4-hx-render-help! frame panel)
  (let ([style (components.style-with-dim
                (components.style-fg (components.style) components.Color/Gray))])
    (lean4-hx-render-panel-text! frame panel 0 "Lean panel help" (components.style-with-bold style))
    (lean4-hx-render-panel-text! frame panel 2
                                 (string-append lean4-hx-leader-label " ?  help") style)
    (lean4-hx-render-panel-text! frame panel 3
                                 (string-append lean4-hx-leader-label " g  go to tagged target") style)
    (lean4-hx-render-panel-text! frame panel 4
                                 (string-append lean4-hx-leader-label " n/p  next/previous goal") style)
    (lean4-hx-render-panel-text! frame panel 5
                                 (string-append lean4-hx-leader-label " t  term goal") style)
    (lean4-hx-render-panel-text! frame panel 6
                                 (string-append lean4-hx-leader-label " a  adaptive layout") style)
    (lean4-hx-render-panel-text! frame panel 7
                                 (string-append lean4-hx-leader-label " y/Y  copy/register") style)
    (lean4-hx-render-panel-text! frame panel 8 "source Space k  hover documentation" style)
    (lean4-hx-render-panel-text! frame panel 9 "Esc  close help" style)))

(define (lean4-hx-report-selection!)
  (if (and lean4-hx-selection-start lean4-hx-selection-end)
      (hx.set-status!
       (string-append
        "lean4.hx selected "
        (number->string
         (+ 1 (- (max lean4-hx-selection-start lean4-hx-selection-end)
                 (min lean4-hx-selection-start lean4-hx-selection-end))))
        " panel rows"))
      (hx.set-status! "lean4.hx: no panel selection")))

(define (lean4-hx-copy-selection! system?)
  (if (and lean4-hx-selection-start lean4-hx-selection-end)
      (let* ([area (if lean4-hx-selection-info?
                       lean4-hx-info-area
                       lean4-hx-goal-area)]
             [origin (if area (components.area-y area) 0)]
             [start (max 0 (- (min lean4-hx-selection-start lean4-hx-selection-end) origin))]
             [end (max 0 (- (max lean4-hx-selection-start lean4-hx-selection-end) origin))]
             [text (if lean4-hx-selection-info?
                       (native-selected-info-text lean4-hx-native
                                                   (components.area-width area)
                                                   start end)
                       (native-selected-text lean4-hx-native
                                              (components.area-width area)
                                              start end))])
        (if (string=? text "")
            (hx.set-status! "lean4.hx: empty panel selection")
            (begin
              (editor.set-register! (if system? #\+ #\") (list text))
              (hx.set-status! (if system?
                                 "lean4.hx: copied to clipboard"
                                 "lean4.hx: copied panel text")))))
      (hx.set-status! "lean4.hx: no panel selection")))

(define (lean4-hx-extend-key-selection! delta)
  (let* ([area (if lean4-hx-selection-info? lean4-hx-info-area lean4-hx-goal-area)]
         [origin (if area (components.area-y area) 0)]
         [height (if area (components.area-height area) 1)]
         [current (if lean4-hx-selection-end lean4-hx-selection-end origin)]
         [next (max origin (min (+ origin height -1) (+ current delta)))])
    (if (not lean4-hx-selection-start)
        (set! lean4-hx-selection-start origin)
        #f)
    (set! lean4-hx-selection-end next)))

(define (lean4-hx-render-footer! frame panel)
  (let ([style (components.style-with-dim
                (components.style-fg (components.style) components.Color/Gray))]
        [row (- (components.area-height panel) 1)])
    (if (native-focused? lean4-hx-native)
        (lean4-hx-render-panel-text!
         frame panel row
         (string-append "For help, use " lean4-hx-leader-label " ?") style)
        #f)))

(define (lean4-hx-render state area frame)
  (let* ([width (min lean4-hx-panel-width (components.area-width area))]
         [left (+ (components.area-x area)
                  (- (components.area-width area) width))]
         [panel (components.area left
                                 (components.area-y area)
                                 width
                                 (components.area-height area))]
         [footer-height (if (native-focused? state) 1 0)]
         [content-height (max 0 (- (components.area-height panel) footer-height))]
         [goal-lines (native-goal-line-count state (components.area-width panel))]
         [info-lines (string->jsexpr (native-info-lines state (components.area-width panel)))]
         [half-height (quotient (+ content-height 1) 2)]
         [adaptive? (equal? lean4-hx-layout-mode "adaptive")]
         [goal-height (cond
                       [(= content-height 0) 0]
                       [(and adaptive? (= (length info-lines) 0)) content-height]
                       [(and adaptive? (> goal-lines half-height))
                        (min content-height
                             (max half-height (quotient (* content-height 2) 3)))]
                       [else half-height])]
         [info-height (- content-height goal-height)]
         [goal-panel (components.area left
                                      (components.area-y panel)
                                      width
                                      goal-height)]
         [info-panel (components.area left
                                      (+ (components.area-y panel) goal-height)
                                      width
                                      info-height)])
    ;; Keep the rectangles in Scheme so the dynamic component can hit-test
    ;; mouse input without pretending to be a full Helix editor view.
    (set! lean4-hx-goal-area goal-panel)
    (set! lean4-hx-info-area info-panel)
    (components.buffer/clear-with frame panel (components.style))
    (if lean4-hx-help-visible?
        (lean4-hx-render-help! frame panel)
        (begin
          (if (> goal-height 0)
              (lean4-hx-render-lines
               frame goal-panel
               (string->jsexpr (native-styled-lines state (components.area-width goal-panel))))
              #f)
          (if (> info-height 0)
              (lean4-hx-render-lines
               frame info-panel
               (string->jsexpr (native-info-lines state (components.area-width info-panel))))
              #f)))
    (if (> footer-height 0)
        (lean4-hx-render-footer! frame panel)
        #f)
    #f))

(define (lean4-hx-handle-event state event)
  (cond
   [(and (components.mouse-event? event)
         (or (and lean4-hx-goal-area
                  (components.mouse-event-within-area? event lean4-hx-goal-area))
             (and lean4-hx-info-area
                  (components.mouse-event-within-area? event lean4-hx-info-area))))
    (native-set-focused! state #t)
    (set! lean4-hx-selection-info?
          (and lean4-hx-info-area
               (components.mouse-event-within-area? event lean4-hx-info-area)))
    (cond
     [(= (components.event-mouse-kind event) 0)
      (set! lean4-hx-selection-start (components.event-mouse-row event))
      (set! lean4-hx-selection-end (components.event-mouse-row event))]
     [(= (components.event-mouse-kind event) 6)
      (set! lean4-hx-selection-end (components.event-mouse-row event))]
     [(= (components.event-mouse-kind event) 3)
      (set! lean4-hx-selection-end (components.event-mouse-row event))]
     [(= (components.event-mouse-kind event) 10)
      (native-scroll! state 1)]
     [(= (components.event-mouse-kind event) 11)
      (native-scroll! state -1)]
     [else #f])
    components.event-result/consume]
   [(components.key-event-tab? event)
    (native-set-focused! state #t)
    components.event-result/consume]
   [(components.key-event-escape? event)
    (if lean4-hx-help-visible?
        (begin
          (set! lean4-hx-help-visible? #f)
          components.event-result/consume)
        (begin
          (native-set-focused! state #f)
          (set! lean4-hx-prefix-pending? #f)
          (set! lean4-hx-leader-pending? #f)
          (set! lean4-hx-selection-start #f)
          (set! lean4-hx-selection-end #f)
          (set! lean4-hx-selection-info? #f)
          ;; Let Helix also process Escape so insert mode returns to normal mode.
          components.event-result/ignore))]
   [(components.mouse-event? event)
    ;; A click outside the compositor panel returns ownership to the source
    ;; editor before Helix handles the click.
    (lean4-hx-clear-panel-focus!)
    components.event-result/ignore]
   [(not (native-focused? state)) components.event-result/ignore]
   [(components.key-event-char event)
    (let ([char (components.key-event-char event)])
      (cond
       [(and lean4-hx-leader-pending? (equal? char #\?))
        (set! lean4-hx-prefix-pending? #f)
        (set! lean4-hx-leader-pending? #f)
        (set! lean4-hx-help-visible? #t)
        components.event-result/consume]
       [(and lean4-hx-leader-pending? (equal? char #\g))
        (set! lean4-hx-prefix-pending? #f)
        (set! lean4-hx-leader-pending? #f)
        (lean4-hx-apply-navigation!)
        components.event-result/consume]
       [(and lean4-hx-leader-pending? (equal? char #\n))
        (set! lean4-hx-prefix-pending? #f)
        (set! lean4-hx-leader-pending? #f)
        (native-next-goal! state)
        components.event-result/consume]
       [(and lean4-hx-leader-pending? (equal? char #\p))
        (set! lean4-hx-prefix-pending? #f)
        (set! lean4-hx-leader-pending? #f)
        (native-previous-goal! state)
        components.event-result/consume]
       [(and lean4-hx-leader-pending? (equal? char #\t))
        (set! lean4-hx-prefix-pending? #f)
        (set! lean4-hx-leader-pending? #f)
        (lean4-hx-request-term-goal!)
        components.event-result/consume]
       [(and lean4-hx-leader-pending? (equal? char #\a))
        (set! lean4-hx-prefix-pending? #f)
        (set! lean4-hx-leader-pending? #f)
        (if (equal? lean4-hx-layout-mode "split")
            (set! lean4-hx-layout-mode "adaptive")
            (set! lean4-hx-layout-mode "split"))
        components.event-result/consume]
       [(and lean4-hx-leader-pending? (equal? char #\y))
        (set! lean4-hx-prefix-pending? #f)
        (set! lean4-hx-leader-pending? #f)
        (lean4-hx-copy-selection! #f)
        components.event-result/consume]
       [(and lean4-hx-leader-pending? (equal? char #\Y))
        (set! lean4-hx-prefix-pending? #f)
        (set! lean4-hx-leader-pending? #f)
        (lean4-hx-copy-selection! #t)
        components.event-result/consume]
       [(and lean4-hx-prefix-pending? (equal? char lean4-hx-leader-key))
        (set! lean4-hx-prefix-pending? #f)
        (set! lean4-hx-leader-pending? #t)
        components.event-result/consume]
       [(equal? char #\space)
        (set! lean4-hx-prefix-pending? #t)
        (set! lean4-hx-leader-pending? #f)
        components.event-result/consume]
       [(equal? char #\g)
        (lean4-hx-apply-navigation!)
        components.event-result/consume]
       [(equal? char #\n)
        (native-next-goal! state)
        components.event-result/consume]
       [(equal? char #\p)
        (native-previous-goal! state)
        components.event-result/consume]
       [(equal? char #\t)
        (lean4-hx-request-term-goal!)
        components.event-result/consume]
       [(equal? char #\j)
        (lean4-hx-extend-key-selection! 1)
        components.event-result/consume]
       [(equal? char #\k)
        (lean4-hx-extend-key-selection! -1)
        components.event-result/consume]
       [else components.event-result/ignore]))]
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
              (editor.register-hook 'document-focus-lost lean4-hx-on-focus-lost)
              (editor.register-hook 'on-mode-switch lean4-hx-on-mode-switch)
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
  (set! lean4-hx-session-document #f)
  "lean4.hx component removed")
