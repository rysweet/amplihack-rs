# Gherkin spec for the amplihack-eval crate (Issue #285)
# Ports amplihack.eval.* Python framework to native Rust

Feature: Evaluation framework - benchmark execution, scoring, and reporting
  As an amplihack developer
  I want to run benchmarks, score results, and generate reports
  So that I can measure and compare agent quality over time

  # ─── Benchmark ─────────────────────────────────────────────────────────────

  Scenario: Create a benchmark with a valid name
    Given a benchmark name "my-benchmark"
    When I create a Benchmark
    Then the benchmark name is "my-benchmark"
    And the description is empty

  Scenario: Create a benchmark with a description
    Given a benchmark name "bench"
    And a description "Tests recall capability"
    When I create a Benchmark
    Then the description is "Tests recall capability"

  Scenario: Reject a benchmark with an empty name
    Given a benchmark name ""
    When I create a Benchmark
    Then an InvalidBenchmark error is returned

  Scenario: Record case results and compute statistics
    Given a BenchmarkResult for "stats-bench"
    When I add case "c1" with passed=true score=1.0 duration=10ms
    And I add case "c2" with passed=false score=0.0 duration=20ms
    And I add case "c3" with passed=true score=0.5 duration=30ms
    And I call finish
    Then total is 3
    And passed is 2
    And failed is 1
    And mean_score is 0.5
    And total_duration_ms is 60
    And finished_at is set

  Scenario: Reject a case with score outside [0.0, 1.0]
    Given a CaseResult for "bad-case" with score 1.5
    Then an InvalidScore error is returned

  Scenario: Timer measures elapsed wall-clock time
    Given a Benchmark "timer-bench"
    When I start a timer and stop it immediately
    Then the elapsed milliseconds is a non-negative integer

  # ─── Scorer ─────────────────────────────────────────────────────────────────

  Scenario: All cases pass produces overall_passed=true
    Given a BenchmarkResult "all-pass" with cases [(c1,true,1.0,5),(c2,true,0.8,5)]
    When I score with default config (threshold=0.7)
    Then overall_passed is true
    And pass_rate is 1.0

  Scenario: All cases fail produces overall_passed=false
    Given a BenchmarkResult "all-fail" with cases [(c1,false,0.1,5)]
    When I score with default config (threshold=0.7)
    Then overall_passed is false

  Scenario: Empty result scores zero
    Given an empty BenchmarkResult "empty"
    When I score with default config
    Then total_cases is 0
    And pass_rate is 0.0
    And overall_passed is false

  Scenario: Custom pass threshold is respected
    Given a BenchmarkResult "threshold-test" with cases [(c1,true,0.5,0)]
    When I score with threshold=0.9
    Then overall_passed is false

  Scenario: Composite score formula
    Given a BenchmarkResult "composite" with cases [(c1,true,0.6,0),(c2,false,0.4,0)]
    And scorer config pass_rate_weight=0.5 threshold=0.0
    When I score
    Then composite_score is approximately 0.5

  Scenario: Improvement detected in comparison
    Given baseline RunScore with composite_score=0.3
    And candidate RunScore with composite_score=0.8
    When I compare baseline to candidate
    Then improved is true
    And composite_delta is approximately 0.5

  Scenario: Regression detected in comparison
    Given baseline RunScore with composite_score=0.8
    And candidate RunScore with composite_score=0.3
    When I compare baseline to candidate
    Then improved is false
    And composite_delta is approximately -0.5

  # ─── Reporter ───────────────────────────────────────────────────────────────

  Scenario: JSON report is valid JSON with required fields
    Given a RunScore for benchmark "json-test"
    When I generate a JSON report
    Then the output is valid JSON
    And it contains field "benchmark_name" with value "json-test"
    And it contains field "overall_passed"

  Scenario: Text report contains benchmark name and status
    Given a passing RunScore for benchmark "text-test"
    When I generate a text report
    Then the output contains "text-test"
    And the output contains "PASS"

  Scenario: Text report shows FAIL for failing score
    Given a failing RunScore for benchmark "fail-test"
    When I generate a text report
    Then the output contains "FAIL"

  Scenario: Comparison text report shows improvement direction
    Given an improved RunComparison
    When I generate a comparison text report
    Then the output contains "improved"

  Scenario: Comparison text report shows regression direction
    Given a regressed RunComparison
    When I generate a comparison text report
    Then the output contains "regressed"

  Scenario: Comparison JSON report is valid JSON
    Given a RunComparison
    When I generate a comparison JSON report
    Then the output is valid JSON
    And it contains field "improved"

  # ─── CLI ────────────────────────────────────────────────────────────────────

  Scenario: eval report --format text prints human-readable output
    Given a saved RunScore JSON file at /tmp/test-score.json
    When I run "amplihack eval report /tmp/test-score.json --format text"
    Then stdout contains "Benchmark:"
    And the exit code is 0

  Scenario: eval report --format json prints JSON
    Given a saved RunScore JSON file at /tmp/test-score.json
    When I run "amplihack eval report /tmp/test-score.json --format json"
    Then stdout is valid JSON
    And the exit code is 0

  Scenario: eval run with passing benchmark exits 0
    Given a BenchmarkResult JSON config where all cases pass
    When I run "amplihack eval run <config> --format text"
    Then the exit code is 0

  Scenario: eval run with failing benchmark exits 1
    Given a BenchmarkResult JSON config where all cases fail
    When I run "amplihack eval run <config>"
    Then the exit code is 1

  Scenario: eval compare prints delta between two result files
    Given a baseline RunScore JSON file
    And a candidate RunScore JSON file with higher composite_score
    When I run "amplihack eval compare <baseline> <candidate>"
    Then stdout contains "improved"
    And the exit code is 0

  Scenario: eval run accepts stdin via "-"
    Given a BenchmarkResult JSON on stdin
    When I run "amplihack eval run -"
    Then the result is printed to stdout

  Scenario: eval run with --threshold override
    Given a BenchmarkResult JSON with composite_score 0.6
    When I run "amplihack eval run <config> --threshold 0.9"
    Then the exit code is 1
