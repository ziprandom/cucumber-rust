Feature: Basic functionality

  Scenario: foo
    Given a thing
    When nothing

  Scenario: bar
    Given a thing
    When something goes wrong

  Scenario Outline: fizz
    Given a thing is <key>
    Then it's reverse is <reverse_key>
    And this table makes sense:
      | key   | reverse_key   |
      | <key> | <reverse_key> |
    And this docstring makes sense:
      """
      the reverse of <key> is <reverse_key>
      """

    Examples:
      | key   | reverse_key |
      | name  | eman        |
      | otto  | otto        |

  Rule: A rule

    Scenario: a scenario inside a rule
      Given I am in inside a rule
      Then things are working
