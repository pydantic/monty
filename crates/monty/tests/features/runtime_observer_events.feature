Feature: Generic runtime observer events

  Scenario: Function call emits request and return events
    Given a suspendable script with one external function call
    When execution starts with a recording observer and resumes with integer return value
    Then observer events include an external function request
    And observer events include an external function return

  Scenario: Branching code emits control and operation-result events
    Given a script with arithmetic and branch control flow
    When execution starts with a recording observer and runs to completion
    Then observer events include a control condition event
    And observer events include an operation-result event with inputs

  Scenario: Failed external call emits error return event
    Given a suspendable script with one external function call
    When execution starts with a recording observer and resumes with raised exception
    Then observer events include an external error return
