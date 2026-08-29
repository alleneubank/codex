use super::invalid_request;
use codex_app_server_protocol::JSONRPCErrorError;
use codex_app_server_protocol::TurnWithdrawPendingInputError;
use codex_app_server_protocol::TurnWithdrawPendingInputErrorReason;
use codex_app_server_protocol::TurnWithdrawPendingInputResponse;
use codex_core::WithdrawPendingInputResult;

pub(super) fn map_withdraw_pending_input_result(
    expected_turn_id: &str,
    result: WithdrawPendingInputResult,
) -> Result<TurnWithdrawPendingInputResponse, JSONRPCErrorError> {
    match result {
        WithdrawPendingInputResult::Withdrawn { turn_id } => {
            Ok(TurnWithdrawPendingInputResponse { turn_id })
        }
        WithdrawPendingInputResult::NoActiveTurn => Err(withdrawal_error(
            "no active turn contains pending input",
            TurnWithdrawPendingInputError {
                reason: TurnWithdrawPendingInputErrorReason::NoActiveTurn,
                expected_turn_id: Some(expected_turn_id.to_string()),
                actual_turn_id: None,
            },
        )),
        WithdrawPendingInputResult::ExpectedTurnMismatch { expected, actual } => {
            let message = format!("expected active turn id `{expected}` but found `{actual}`");
            Err(withdrawal_error(
                message,
                TurnWithdrawPendingInputError {
                    reason: TurnWithdrawPendingInputErrorReason::ExpectedTurnMismatch,
                    expected_turn_id: Some(expected),
                    actual_turn_id: Some(actual),
                },
            ))
        }
        WithdrawPendingInputResult::NotPending { turn_id } => Err(withdrawal_error(
            "client user message id is not pending",
            TurnWithdrawPendingInputError {
                reason: TurnWithdrawPendingInputErrorReason::NotPending,
                expected_turn_id: Some(expected_turn_id.to_string()),
                actual_turn_id: Some(turn_id),
            },
        )),
        WithdrawPendingInputResult::AmbiguousClientId { turn_id } => Err(withdrawal_error(
            "client user message id matches multiple pending inputs",
            TurnWithdrawPendingInputError {
                reason: TurnWithdrawPendingInputErrorReason::AmbiguousClientUserMessageId,
                expected_turn_id: Some(expected_turn_id.to_string()),
                actual_turn_id: Some(turn_id),
            },
        )),
    }
}

fn withdrawal_error(
    message: impl Into<String>,
    data: TurnWithdrawPendingInputError,
) -> JSONRPCErrorError {
    let mut error = invalid_request(message);
    error.data = Some(
        serde_json::to_value(data)
            .expect("pending-input withdrawal error contains only serializable values"),
    );
    error
}
