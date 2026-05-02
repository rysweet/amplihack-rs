//! Expert role definitions for the expert-panel pattern.

/// One expert profile.
#[derive(Debug, Clone, Copy)]
pub struct Expert {
    pub domain: &'static str,
    pub focus: &'static str,
}

/// Default 3 experts: security, performance, simplicity.
pub static DEFAULT_EXPERTS: &[Expert] = &[
    Expert {
        domain: "security",
        focus: "vulnerabilities, attack vectors, data protection, security best practices",
    },
    Expert {
        domain: "performance",
        focus: "speed, scalability, resource efficiency, latency, throughput",
    },
    Expert {
        domain: "simplicity",
        focus: "minimal complexity, ruthless simplification, maintainability, clarity",
    },
];

/// Build the per-expert review prompt.
pub fn build_review_prompt(solution: &str, expert: &Expert, total: usize) -> String {
    format!(
        "You are an expert reviewer participating in an Expert Panel Review.\n\n\
         SOLUTION TO REVIEW:\n{solution}\n\n\
         YOUR EXPERTISE: {domain_upper}\nFOCUS AREAS: {focus}\n\n\
         Your task is to perform an independent expert review and cast a vote.\n\n\
         IMPORTANT:\n\
         - You are ONE expert among multiple (total: {total})\n\
         - Your review is INDEPENDENT (do not consider other experts)\n\
         - Focus ONLY on your domain of expertise\n\
         - Use your confidence score to express uncertainty (0.0 - 1.0)\n\n\
         FORMAT YOUR RESPONSE EXACTLY AS:\n\n\
         ## Analysis\n[Your detailed analysis from {domain} perspective]\n\n\
         ## Strengths\n- [Strength 1]\n- [Strength 2]\n\n\
         ## Weaknesses\n- [Weakness 1]\n\n\
         ## Domain Scores\n- aspect_name_1: 0.8\n\n\
         ## Vote\n[APPROVE or REJECT or ABSTAIN]\n\n\
         ## Confidence\n[Number between 0.0 and 1.0]\n\n\
         ## Vote Rationale\n[Clear explanation]\n",
        domain_upper = expert.domain.to_uppercase(),
        domain = expert.domain,
        focus = expert.focus,
        solution = solution,
        total = total,
    )
}

#[cfg(test)]
mod inline_tests {
    use super::*;

    #[test]
    fn default_experts_have_three() {
        assert_eq!(DEFAULT_EXPERTS.len(), 3);
    }

    #[test]
    fn review_prompt_contains_uppercase_domain() {
        let p = build_review_prompt("code", &DEFAULT_EXPERTS[0], 3);
        assert!(p.contains("YOUR EXPERTISE: SECURITY"));
    }
}
